// Host side of the live mirror (ADR 0006): talk to the in-game shims, read the mirror
// file, keep a live session going. What the mirrored bytes mean is the game's business:
// a game passes a `discover` function (which regions to mirror) and a `decode` function.
//
// Linux only: the host is the native side of the Wine boundary.

import Foundation
import Glibc
import GraceProto
import Synchronization

private typealias Instant = ContinuousClock.Instant

private let staleAfter: Duration = .milliseconds(250)
private let rediscoverAfter: Duration = .seconds(1)
private let retryAfter: Duration = .seconds(2)
private let maxArrivals = 512
private let maxAges = 240

/// MSG_NOSIGNAL. Spelled as a number because Glibc's anonymous C enums import unevenly.
private let noSignal: Int32 = 0x4000
/// Memory walks take tens of seconds.
private let readTimeoutMs: Int32 = 120_000

private func lastOSError() -> GraceError {
    GraceError(String(cString: strerror(errno)))
}

private func environmentPort(_ name: String, default fallback: UInt16) -> UInt16 {
    getenv(name).flatMap { UInt16(String(cString: $0)) } ?? fallback
}

/// Control port of the dxgi shim (frame stats, live mirror).
public func dxgiPort() -> UInt16 {
    environmentPort("GRACE_SHIM_PORT", default: 47321)
}

/// Control port of the d3d12 shim (memory search, overrides).
public func d3d12Port() -> UInt16 {
    environmentPort("GRACE_D3D12_PORT", default: 47322)
}

/// Move-only, so the descriptor closes exactly once: Swift's answer to `OwnedFd`.
struct OwnedDescriptor: ~Copyable {
    let raw: Int32

    init(_ raw: Int32) {
        self.raw = raw
    }

    deinit {
        _ = Glibc.close(raw)
    }
}

private func connectLoopback(port: UInt16) throws -> OwnedDescriptor {
    let context = "shim on 127.0.0.1:\(port): is the game running under the oracle?"

    // SOCK_STREAM is a C enum on Linux and an Int32 macro on Darwin. Hence `.rawValue`.
    let raw = Glibc.socket(AF_INET, Int32(SOCK_STREAM.rawValue) | Int32(SOCK_CLOEXEC.rawValue), 0)
    guard raw >= 0 else {
        throw lastOSError().context(context)
    }
    let descriptor = OwnedDescriptor(raw)

    var address = sockaddr_in()
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = port.bigEndian
    address.sin_addr.s_addr = UInt32(0x7F00_0001).bigEndian
    let status = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Glibc.connect(raw, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    guard status == 0 else {
        throw lastOSError().context(context)
    }
    return descriptor
}

/// A blocking client of a shim control port (`grace-proto`, one JSON line per message).
public struct ShimClient: ~Copyable {
    private let socket: OwnedDescriptor
    private let codec: any WireFormat
    private var buffer: [UInt8] = []

    public init(port: UInt16, codec: any WireFormat) throws {
        socket = try connectLoopback(port: port)
        self.codec = codec
    }

    public mutating func ask(_ request: Request) throws -> Response {
        try sendLine(codec.encode(request))
        guard let line = try nextLine() else {
            throw GraceError("shim closed the connection")
        }
        let response = try codec.decode(line)
        if case .error(let message) = response {
            throw GraceError("shim: \(message)")
        }
        return response
    }

    private func sendLine(_ line: String) throws {
        let bytes = Array((line + "\n").utf8)
        let raw = socket.raw
        var sent = 0
        while sent < bytes.count {
            // MSG_NOSIGNAL: Rust's runtime ignores SIGPIPE before `main`, Swift's does not.
            let count = bytes.withUnsafeBytes { Glibc.send(raw, $0.baseAddress! + sent, $0.count - sent, noSignal) }
            if count < 0 {
                if errno == EINTR {
                    continue
                }
                throw lastOSError()
            }
            sent += count
        }
    }

    /// `poll` instead of SO_RCVTIMEO: on recent kernel headers that constant is a `sizeof`
    /// expression, and Swift cannot import a macro like that.
    private func waitUntilReadable() throws {
        var descriptor = pollfd(fd: socket.raw, events: Int16(POLLIN), revents: 0)
        while true {
            let ready = Glibc.poll(&descriptor, 1, readTimeoutMs)
            if ready > 0 {
                return
            }
            if ready == 0 {
                throw GraceError("timed out waiting for the shim")
            }
            if errno != EINTR {
                throw lastOSError()
            }
        }
    }

    private mutating func nextLine() throws -> String? {
        let raw = socket.raw
        var chunk = [UInt8](repeating: 0, count: 4096)
        while true {
            if let newline = buffer.firstIndex(of: 0x0A) {
                let line = String(decoding: buffer[..<newline], as: UTF8.self)
                buffer.removeSubrange(...newline)
                return line
            }
            try waitUntilReadable()
            let got = chunk.withUnsafeMutableBytes { Glibc.recv(raw, $0.baseAddress, $0.count, 0) }
            if got < 0 {
                if errno == EINTR {
                    continue
                }
                throw lastOSError()
            }
            if got == 0 {
                guard !buffer.isEmpty else {
                    return nil
                }
                defer { buffer.removeAll() }
                return String(decoding: buffer, as: UTF8.self)
            }
            buffer.append(contentsOf: chunk[..<got])
        }
    }
}

/// The mirror file mapped read-only.
///
/// `@unchecked` is Swift's `unsafe impl Send + Sync`: the map is immutable from this side
/// and reads go through the seqlock.
public final class MirrorReader: @unchecked Sendable {
    private let map: UnsafeRawPointer

    public init(file: URL) throws {
        let path = file.path
        let raw = Glibc.open(path, O_RDONLY | O_CLOEXEC)
        guard raw >= 0 else {
            throw lastOSError().context("mirror file \(path)")
        }
        let descriptor = OwnedDescriptor(raw)

        var info = stat()
        guard fstat(raw, &info) == 0 else {
            throw lastOSError()
        }
        guard info.st_size >= MirrorRecord.size else {
            throw GraceError("\(path) is not a mirror record yet (the shim creates it at the first frame)")
        }
        // The shim only writes inside the record; reads go through the seqlock.
        guard let mapped = mmap(nil, MirrorRecord.size, PROT_READ, MAP_SHARED, descriptor.raw, 0),
              mapped != UnsafeMutableRawPointer(bitPattern: -1)
        else {
            throw lastOSError()
        }
        map = UnsafeRawPointer(mapped)
    }

    deinit {
        munmap(UnsafeMutableRawPointer(mutating: map), MirrorRecord.size)
    }

    public var seq: UInt64 {
        MirrorRecord.currentSeq(map)
    }

    public func snapshot() -> Snapshot? {
        MirrorRecord.read(map)
    }
}

/// Session state reported to the caller every frame.
public enum LiveStatus<State> {
    /// Looking for what to mirror (memory search in progress or retrying).
    case searching(String)
    /// A state decoded from the latest game frame.
    case fresh(State)
    /// The game has not produced a frame for a while (paused, minimized); the last state.
    case stale(State)
}

/// Arrival times of new mirror frames, stamped by a 1 kHz watcher with the host clock
/// (so sample age needs no mapping between Wine's QPC and the host clock).
private struct Arrival: Sendable {
    let seq: UInt64
    let at: Instant
}

private final class Arrivals: Sendable {
    let frames = Mutex<[Arrival]>([])  // no Deque without swift-collections
}

private final class SearchOutcome: Sendable {
    private let slot = Mutex<Result<[MirrorRegion], any Error>?>(nil)

    var result: Result<[MirrorRegion], any Error>? {
        slot.withLock { $0 }
    }

    func finish(_ result: Result<[MirrorRegion], any Error>) {
        slot.withLock { $0 = result }
    }
}

private struct Search {
    let outcome: SearchOutcome
    let task: Task<Void, Never>
}

/// Runs off the render thread. May do blocking work, at the price of a cooperative-pool thread.
public typealias Discover = @Sendable () async throws -> [MirrorRegion]

/// A live session: discovery on a background task, mirroring, decoding, rediscovery
/// when the mirrored bytes stop decoding, and stats.
///
/// Deliberately not `Sendable`: one owner polls it, and the compiler holds you to that.
public final class LiveSession<State> {
    private let reader: MirrorReader
    private let arrivals: Arrivals
    private let codec: any WireFormat
    private let discover: Discover
    private let decode: (Snapshot) -> State?
    private let watcher: Task<Void, Never>

    private var search: Search?
    private var retryAt: Instant?
    public private(set) var note = "starting"
    private var generation: UInt32?
    private var last: State?
    private var lastGood: Instant?
    private var ages: [Double] = []
    private var copyNs: UInt32 = 0

    public init(
        mirrorFile: URL,
        codec: any WireFormat,
        discover: @escaping Discover,
        decode: @escaping (Snapshot) -> State?
    ) throws {
        let reader = try MirrorReader(file: mirrorFile)
        let arrivals = Arrivals()
        self.reader = reader
        self.arrivals = arrivals
        self.codec = codec
        self.discover = discover
        self.decode = decode
        watcher = Task.detached(priority: .high) {
            var last: UInt64 = 0
            while !Task.isCancelled {
                let seq = reader.seq
                if seq != last && seq % 2 == 0 {
                    last = seq
                    let arrival = Arrival(seq: seq, at: .now)
                    arrivals.frames.withLock { frames in
                        frames.append(arrival)
                        if frames.count > maxArrivals {
                            frames.removeFirst()
                        }
                    }
                }
                try? await Task.sleep(for: .milliseconds(1))
            }
        }
    }

    deinit {
        watcher.cancel()
        search?.task.cancel()
    }

    /// Call once per rendered frame.
    public func poll() -> LiveStatus<State> {
        guard let generation else {
            return pollSearch()
        }
        guard let snapshot = reader.snapshot() else {
            return searching("mirror record unreadable")
        }
        guard snapshot.info.generation == generation else {
            // The shim has not switched to our region set yet.
            return staleOrSearching("waiting for the first mirrored frame")
        }
        copyNs = snapshot.info.copyNs
        let arrivedAt = arrived(snapshot.seq)

        guard let state = decode(snapshot) else {
            let framesMove = arrivedAt.map { Instant.now - $0 < staleAfter } ?? false
            let goneBad = lastGood.map { Instant.now - $0 > rediscoverAfter } ?? true
            if framesMove && goneBad {
                self.generation = nil
                lastGood = nil
                return searching("mirrored bytes stopped decoding, searching again")
            }
            return staleOrSearching("decoding")
        }

        let now = Instant.now
        last = state
        lastGood = now
        guard let arrival = arrivedAt else {
            return .fresh(state)
        }
        let age = now - arrival
        ages.append(age.inMilliseconds)
        if ages.count > maxAges {
            ages.removeFirst()
        }
        return age > staleAfter ? .stale(state) : .fresh(state)
    }

    private func pollSearch() -> LiveStatus<State> {
        if let result = search?.outcome.result {
            search = nil
            do {
                let regions = try result.get()
                generation = try register(regions)
                note = "mirroring \(regions.count) regions"
                return staleOrSearching("waiting for the first mirrored frame")
            } catch {
                note = String(describing: error)
                retryAt = Instant.now + retryAfter
            }
        }

        let retryDue = retryAt.map { Instant.now >= $0 } ?? true
        if search == nil && retryDue {
            startSearch()
            retryAt = nil
            note = "searching game memory"
        }
        if let last {
            return .stale(last)
        }
        return .searching(note)
    }

    private func startSearch() {
        let outcome = SearchOutcome()
        let discover = self.discover
        let task = Task.detached {
            do {
                let regions = try await discover()
                outcome.finish(.success(regions))
            } catch {
                outcome.finish(.failure(error))
            }
        }
        search = Search(outcome: outcome, task: task)
    }

    private func register(_ regions: [MirrorRegion]) throws -> UInt32 {
        var client = try ShimClient(port: dxgiPort(), codec: codec)
        let reply = try client.ask(.mirror(regions: regions))
        guard case .mirror(let generation) = reply else {
            throw GraceError("unexpected reply \(reply)")
        }
        return generation
    }

    private func searching(_ why: String) -> LiveStatus<State> {
        note = why
        return .searching(why)
    }

    private func staleOrSearching(_ why: String) -> LiveStatus<State> {
        if let last {
            return .stale(last)
        }
        return searching(why)
    }

    private func arrived(_ seq: UInt64) -> Instant? {
        arrivals.frames.withLock { frames in
            frames.last { $0.seq == seq }?.at
        }
    }

    /// Game frames per second over the last second of arrivals.
    public var gameFPS: Double {
        arrivals.frames.withLock { frames in
            let now = Instant.now
            return Double(frames.filter { now - $0.at < .seconds(1) }.count)
        }
    }

    /// Sample age at use, ms: (p50, p99) over the last few seconds.
    public var ageMs: (p50: Double, p99: Double)? {
        guard !ages.isEmpty else {
            return nil
        }
        let sorted = ages.sorted()
        func at(_ quantile: Double) -> Double {
            sorted[Int((Double(sorted.count - 1) * quantile).rounded())]
        }
        return (at(0.5), at(0.99))
    }

    /// Shim copy time per frame, µs.
    public var shimMicroseconds: Double {
        Double(copyNs) / 1000
    }
}

extension Duration {
    fileprivate var inMilliseconds: Double {
        let (seconds, attoseconds) = components
        return Double(seconds) * 1000 + Double(attoseconds) / 1e15
    }
}

/// Where hosts and `xtask oracle run` put the mirror file of a game.
public func mirrorFile(localDir: URL, game: String) -> URL {
    localDir.appendingPathComponent("run").appendingPathComponent(game).appendingPathComponent("mirror.bin")
}
