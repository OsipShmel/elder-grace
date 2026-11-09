// Ports of the Rust unit tests, plus end-to-end tests of LiveSession against a fake shim.

import Foundation
import Glibc
import Synchronization
import Testing

@testable import GraceLive
@testable import GraceProto

/// A test-only wire format: the real one lives in grace-proto.
private struct LineFormat: WireFormat {
    func encode(_ request: Request) throws -> String {
        switch request {
        case .mirror(let regions): "mirror \(regions.count)"
        }
    }

    func decode(_ line: String) throws -> Response {
        if line.hasPrefix("generation "), let generation = UInt32(line.dropFirst(11)) {
            return .mirror(generation: generation)
        }
        if line.hasPrefix("error ") {
            return .error(message: String(line.dropFirst(6)))
        }
        return .other(debug: line)
    }
}

/// A record in plain memory. `@unchecked` for the same reason as in production.
private final class Record: @unchecked Sendable {
    let base: UnsafeMutableRawPointer

    init() {
        base = .allocate(byteCount: MirrorRecord.size, alignment: 8)
        base.initializeMemory(as: UInt8.self, repeating: 0, count: MirrorRecord.size)
    }

    deinit {
        base.deallocate()
    }
}

private func fill(_ destination: UnsafeMutableRawBufferPointer, with value: UInt8) {
    for index in destination.indices {
        destination[index] = value
    }
}

@Test func roundtrip() throws {
    let record = Record()
    #expect(MirrorRecord.read(record.base) == nil, "an empty record has no snapshot")

    let regions = [MirrorRegion(addr: 0x1000, len: 5), MirrorRegion(addr: 0x2000, len: 3, chain: [8])]
    let offsets = try MirrorRecord.layout(regions)
    let info = FrameInfo(frame: 7, qpc: 123, qpcFrequency: 10_000_000, copyNs: 900, generation: 2)
    MirrorRecord.write(to: record.base, regions: regions, offsets: offsets, info: info) { index, destination in
        fill(destination, with: UInt8(index + 1))
        return index == 0 ? 0x1000 : nil
    }

    let snapshot = try #require(MirrorRecord.read(record.base))
    #expect(snapshot.seq == 2)
    #expect(snapshot.info == info)
    #expect(snapshot.regions[0] == RegionData(addr: 0x1000, ok: true, bytes: [1, 1, 1, 1, 1]))
    #expect(snapshot.regions[1] == RegionData(addr: 0x2000, ok: false, bytes: [2, 2, 2]))

    MirrorRecord.write(to: record.base, regions: regions, offsets: offsets, info: info) { _, _ in 0x1000 }
    #expect(MirrorRecord.currentSeq(record.base) == 4)
}

@Test func tornRecordIsNotReturned() {
    let record = Record()
    MirrorRecord.write(to: record.base, regions: [], offsets: [], info: FrameInfo()) { _, _ in nil }
    record.base.storeBytes(of: UInt64(3), toByteOffset: MirrorRecord.seqOffset, as: UInt64.self)  // mid-frame
    #expect(MirrorRecord.read(record.base) == nil)
}

@Test func limits() {
    let capacity = UInt32(MirrorRecord.size - MirrorRecord.dataOffset)
    let tooMany = Array(repeating: MirrorRegion(addr: 0, len: 1), count: MirrorRecord.maxRegions + 1)
    #expect(throws: GraceError.self) { try MirrorRecord.layout(tooMany) }
    #expect(throws: Never.self) { try MirrorRecord.layout([MirrorRegion(addr: 0, len: capacity)]) }
    #expect(throws: GraceError.self) { try MirrorRecord.layout([MirrorRegion(addr: 0, len: capacity + 1)]) }
}

@Test func concurrentReadsAreConsistent() async throws {
    let record = Record()
    let regions = [MirrorRegion(addr: 0, len: 4096)]
    let offsets = try MirrorRecord.layout(regions)
    let writer = Task.detached {
        for frame in UInt64(1)...20_000 {
            MirrorRecord.write(to: record.base, regions: regions, offsets: offsets, info: FrameInfo(frame: frame)) {
                _, destination in
                fill(destination, with: UInt8(truncatingIfNeeded: frame))
                return 0
            }
        }
    }

    var seen = 0
    while !Task.isCancelled {
        if let snapshot = MirrorRecord.read(record.base) {
            let expected = UInt8(truncatingIfNeeded: snapshot.info.frame)
            #expect(snapshot.regions[0].bytes.allSatisfy { $0 == expected }, "torn snapshot")
            seen += 1
            if snapshot.info.frame == 20_000 {
                break
            }
        }
    }
    await writer.value
    #expect(seen > 0)
}

@Test func emptyChainIsNotSerialized() throws {
    let encoder = JSONEncoder()
    encoder.outputFormatting = .sortedKeys
    let text = String(decoding: try encoder.encode(MirrorRegion(addr: 1, len: 2)), as: UTF8.self)
    #expect(text == #"{"addr":1,"len":2}"#)
    let decoded = try JSONDecoder().decode(MirrorRegion.self, from: Data(text.utf8))
    #expect(decoded.chain.isEmpty)
}

/// A writer standing in for the shim: maps the same file read-write.
private final class FakeGame {
    let directory: URL
    let file: URL
    private let base: UnsafeMutableRawPointer
    private var frameCount: UInt64 = 0

    init() throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("grace-live-test-\(UUID())")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        file = directory.appendingPathComponent("mirror.bin")
        let raw = Glibc.open(file.path, O_RDWR | O_CREAT, 0o600)
        try #require(raw >= 0)
        defer { _ = Glibc.close(raw) }
        try #require(ftruncate(raw, off_t(MirrorRecord.size)) == 0)
        let mapped = try #require(mmap(nil, MirrorRecord.size, PROT_READ | PROT_WRITE, MAP_SHARED, raw, 0))
        try #require(mapped != UnsafeMutableRawPointer(bitPattern: -1))
        base = mapped
    }

    deinit {
        munmap(base, MirrorRecord.size)
        try? FileManager.default.removeItem(at: directory)
    }

    func frame(generation: UInt32, payload: [UInt8]) throws {
        let regions = [MirrorRegion(addr: 0x10, len: 4)]
        let offsets = try MirrorRecord.layout(regions)
        frameCount += 1
        let info = FrameInfo(frame: frameCount, copyNs: 1500, generation: generation)
        MirrorRecord.write(to: base, regions: regions, offsets: offsets, info: info) { _, destination in
            destination.copyBytes(from: payload)
            return 0x10
        }
    }
}

/// Answers every line with `reply`, on an ephemeral port published through GRACE_SHIM_PORT.
private final class FakeShim: Sendable {
    private let listener: Int32
    private let reply: String
    private let served = Mutex(0)

    var requests: Int {
        served.withLock { $0 }
    }

    init(reply: String) throws {
        self.reply = reply
        listener = Glibc.socket(AF_INET, Int32(SOCK_STREAM.rawValue), 0)

        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_addr.s_addr = UInt32(0x7F00_0001).bigEndian
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let fd = listener
        let status = withUnsafeMutablePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { generic -> Int32 in
                guard Glibc.bind(fd, generic, length) == 0, Glibc.listen(fd, 8) == 0 else {
                    return -1
                }
                return getsockname(fd, generic, &length)
            }
        }
        try #require(status == 0)
        setenv("GRACE_SHIM_PORT", String(UInt16(bigEndian: address.sin_port)), 1)
        Thread.detachNewThread { self.serve() }
    }

    deinit {
        _ = Glibc.close(listener)
    }

    func stop() {
        Glibc.shutdown(listener, Int32(SHUT_RDWR))
    }

    private func serve() {
        while true {
            let client = Glibc.accept(listener, nil, nil)
            guard client >= 0 else {
                return
            }
            var byte: UInt8 = 0
            while Glibc.recv(client, &byte, 1, 0) == 1 && byte != 0x0A {}
            let line = Array((reply + "\n").utf8)
            _ = Glibc.send(client, line, line.count, 0x4000)
            served.withLock { $0 += 1 }
            _ = Glibc.close(client)
        }
    }
}

private func firstByte(_ snapshot: Snapshot) -> Int? {
    let value = Int(snapshot.regions[0].bytes[0])
    return value == 0xFF ? nil : value
}

private func makeSession(_ game: FakeGame, discover: @escaping Discover = { [MirrorRegion(addr: 0x10, len: 4)] }) throws
    -> LiveSession<Int>
{
    try LiveSession(mirrorFile: game.file, codec: LineFormat(), discover: discover, decode: firstByte)
}

private func isSearching(_ status: LiveStatus<Int>, _ why: String) -> Bool {
    if case .searching(why) = status {
        return true
    }
    return false
}

private func pollUntil(_ session: LiveSession<Int>, _ done: (LiveStatus<Int>) -> Bool) async throws {
    for _ in 0..<5000 {
        if done(session.poll()) {
            return
        }
        try await Task.sleep(for: .milliseconds(1))
    }
    Issue.record("timed out, note: \(session.note)")
}

// Serialized: GRACE_SHIM_PORT and GRACE_D3D12_PORT are process-wide.
@Suite(.serialized) struct Live {
    @Test func readerSeesWriterThroughTheFile() throws {
        let game = try FakeGame()
        let reader = try MirrorReader(file: game.file)
        #expect(reader.snapshot() == nil)
        try game.frame(generation: 1, payload: [1, 2, 3, 4])
        let snapshot = try #require(reader.snapshot())
        #expect(snapshot.info.frame == 1)
        #expect(snapshot.regions[0].bytes == [1, 2, 3, 4])
    }

    @Test func mirrorFileLayout() {
        let local = URL(fileURLWithPath: "/l", isDirectory: true)
        #expect(mirrorFile(localDir: local, game: "g").path == "/l/run/g/mirror.bin")
    }

    @Test func missingMirrorFileReportsItsPath() {
        let missing = URL(fileURLWithPath: "/nonexistent/mirror.bin")
        #expect { _ = try MirrorReader(file: missing) } throws: { error in
            String(describing: error).hasPrefix("mirror file /nonexistent/mirror.bin: ")
        }
    }

    @Test func portsComeFromTheEnvironment() {
        setenv("GRACE_D3D12_PORT", "1234", 1)
        #expect(d3d12Port() == 1234)
        for bad in ["12x", "70000", "", "-1"] {
            setenv("GRACE_D3D12_PORT", bad, 1)
            #expect(d3d12Port() == 47322)
        }
        unsetenv("GRACE_D3D12_PORT")
        #expect(d3d12Port() == 47322)
    }

    @Test func connectErrorHasContext() {
        #expect {
            _ = try ShimClient(port: 1, codec: LineFormat())  // nothing listens on port 1
        } throws: { error in
            String(describing: error).hasPrefix("shim on 127.0.0.1:1: is the game running under the oracle?: ")
        }
    }

    @Test func sessionGoesFreshStaleAndRediscovers() async throws {
        let game = try FakeGame()
        let shim = try FakeShim(reply: "generation 5")
        defer { shim.stop() }
        let session = try makeSession(game)
        #expect(session.note == "starting")

        let waiting = "waiting for the first mirrored frame"
        try await pollUntil(session) { isSearching($0, waiting) }
        #expect(shim.requests == 1)

        try game.frame(generation: 4, payload: [9, 0, 0, 0])  // the shim has not switched yet
        #expect(isSearching(session.poll(), waiting))

        try game.frame(generation: 5, payload: [42, 0, 0, 0])
        try await Task.sleep(for: .milliseconds(50))  // let the watcher stamp the frame
        guard case .fresh(42) = session.poll() else {
            Issue.record("expected fresh 42")
            return
        }
        #expect(session.shimMicroseconds == 1.5)
        #expect(session.gameFPS >= 1)
        let ages = try #require(session.ageMs)
        #expect(ages.p50 >= 0 && ages.p50 <= ages.p99)

        try await Task.sleep(for: .milliseconds(300))
        guard case .stale(42) = session.poll() else {
            Issue.record("expected stale 42")
            return
        }

        try await Task.sleep(for: .milliseconds(1100))  // longer than rediscoverAfter
        try game.frame(generation: 5, payload: [0xFF, 0, 0, 0])
        try await Task.sleep(for: .milliseconds(50))
        let again = "mirrored bytes stopped decoding, searching again"
        #expect(isSearching(session.poll(), again))
        #expect(session.note == again)

        // Rediscovery keeps serving the last state.
        guard case .stale(42) = session.poll() else {
            Issue.record("expected stale 42 during rediscovery")
            return
        }
        #expect(session.note == "searching game memory")
        try await pollUntil(session) { _ in session.note == "mirroring 1 regions" }
        #expect(shim.requests == 2)
    }

    @Test(arguments: [
        ("error boom", "shim: boom"),
        ("Stats{..}", #"unexpected reply other(debug: "Stats{..}")"#),
    ])
    func errorRepliesEndUpInTheNote(reply: String, expected: String) async throws {
        let game = try FakeGame()
        let shim = try FakeShim(reply: reply)
        defer { shim.stop() }
        let session = try makeSession(game)
        try await pollUntil(session) { _ in session.note == expected }
    }

    @Test func discoveryFailuresAreReported() async throws {
        let game = try FakeGame()
        let session = try makeSession(game, discover: { throw GraceError("no signature") })
        try await pollUntil(session) { _ in session.note == "no signature" }
    }
}
