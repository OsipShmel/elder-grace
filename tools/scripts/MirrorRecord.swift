// The live mirror record: a fixed 64 KiB block shared between the shim (writer, inside the
// game process) and a native reader through a mapped file. A seqlock in the header gives
// consistent snapshots: the writer makes `seq` odd while it writes, the reader retries on
// an odd or changed `seq`. See ADR 0006.

import Atomics

/// A memory region of the game process to copy every frame.
///
/// Without `chain` the region is at `addr`. With a chain the shim follows pointers every frame:
/// `p = addr; for off in chain { p = *(p as *const u64) + off }`, so `addr` is where the first
/// pointer lives (for example a static in the game's image) and the last offset points at the
/// data. The record's table holds the resolved address.
public struct MirrorRegion: Hashable, Sendable, Codable {
    public var addr: UInt64
    public var len: UInt32
    public var chain: [Int64]

    public init(addr: UInt64, len: UInt32, chain: [Int64] = []) {
        self.addr = addr
        self.len = len
        self.chain = chain
    }

    // `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, the long way round.
    private enum CodingKeys: String, CodingKey {
        case addr, len, chain
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        addr = try container.decode(UInt64.self, forKey: .addr)
        len = try container.decode(UInt32.self, forKey: .len)
        chain = try container.decodeIfPresent([Int64].self, forKey: .chain) ?? []
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(addr, forKey: .addr)
        try container.encode(len, forKey: .len)
        if !chain.isEmpty {
            try container.encode(chain, forKey: .chain)
        }
    }
}

/// Per-frame header values written by the shim.
public struct FrameInfo: Hashable, Sendable {
    /// `Present` count.
    public var frame: UInt64
    /// QueryPerformanceCounter at the copy and its frequency.
    public var qpc: Int64
    public var qpcFrequency: Int64
    /// Time the shim spent copying, ns.
    public var copyNs: UInt32
    /// Bumps every time the region set changes.
    public var generation: UInt32

    public init(frame: UInt64 = 0, qpc: Int64 = 0, qpcFrequency: Int64 = 0, copyNs: UInt32 = 0, generation: UInt32 = 0) {
        self.frame = frame
        self.qpc = qpc
        self.qpcFrequency = qpcFrequency
        self.copyNs = copyNs
        self.generation = generation
    }
}

public struct RegionData: Hashable, Sendable {
    public var addr: UInt64
    /// False when the shim could not read the region (freed or unmapped memory).
    public var ok: Bool
    public var bytes: [UInt8]

    public init(addr: UInt64, ok: Bool, bytes: [UInt8]) {
        self.addr = addr
        self.ok = ok
        self.bytes = bytes
    }
}

public struct Snapshot: Hashable, Sendable {
    public var seq: UInt64
    public var info: FrameInfo
    public var regions: [RegionData]

    public init(seq: UInt64, info: FrameInfo, regions: [RegionData]) {
        self.seq = seq
        self.info = info
        self.regions = regions
    }
}

/// Not called `Mirror`: the standard library already took that name for reflection.
public enum MirrorRecord {
    public static let magic = Array("GRMIRROR".utf8)
    public static let version: UInt32 = 1
    /// Size of the record (and of the mirror file).
    public static let size = 64 * 1024
    public static let maxRegions = 64

    static let seqOffset = 16
    static let tableOffset = 64
    static let entrySize = 16
    static let dataOffset = tableOffset + maxRegions * entrySize

    /// Data offsets of the regions inside the record.
    public static func layout(_ regions: [MirrorRegion]) throws -> [Int] {
        guard regions.count <= maxRegions else {
            throw GraceError("at most \(maxRegions) regions, got \(regions.count)")
        }
        var at = dataOffset
        var offsets: [Int] = []
        offsets.reserveCapacity(regions.count)
        for region in regions {
            offsets.append(at)
            at = (at + Int(region.len) + 15) / 16 * 16
        }
        guard at <= size else {
            throw GraceError("regions need \(at - dataOffset) bytes, the record holds \(size - dataOffset)")
        }
        return offsets
    }

    /// `base` must be 8-aligned and point to `size` bytes that outlive the use.
    private static func seqCell(_ base: UnsafeRawPointer) -> UnsafeAtomic<UInt64> {
        let cell = UnsafeMutableRawPointer(mutating: base + seqOffset)
            .assumingMemoryBound(to: UnsafeAtomic<UInt64>.Storage.self)
        return UnsafeAtomic(at: cell)
    }

    /// Writes one frame. `offsets` comes from `layout` for the same `regions`. `fill(i, dst)`
    /// copies region `i` into `dst` and returns the address it read from (`nil` if the read failed).
    ///
    /// `base` must be 8-aligned and point to `size` writable bytes, and there must be a single writer.
    public static func write(
        to base: UnsafeMutableRawPointer,
        regions: [MirrorRegion],
        offsets: [Int],
        info: FrameInfo,
        fill: (_ index: Int, _ destination: UnsafeMutableRawBufferPointer) -> UInt64?
    ) {
        let seq = seqCell(UnsafeRawPointer(base))
        var s = seq.load(ordering: .relaxed)
        if s % 2 == 1 {
            s += 1  // a previous writer died mid-frame
        }
        seq.store(s + 1, ordering: .relaxed)
        atomicMemoryFence(ordering: .releasing)

        magic.withUnsafeBytes { base.copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
        base.put(version, at: 8)
        base.put(UInt32(regions.count), at: 12)
        base.put(info.frame, at: 24)
        base.put(info.qpc, at: 32)
        base.put(info.qpcFrequency, at: 40)
        base.put(info.copyNs, at: 48)
        base.put(info.generation, at: 52)

        for (i, (region, offset)) in zip(regions, offsets).enumerated() {
            let destination = UnsafeMutableRawBufferPointer(start: base + offset, count: Int(region.len))
            let read = fill(i, destination)
            let entry = base + tableOffset + i * entrySize
            entry.put(read ?? region.addr, at: 0)
            entry.put(region.len, at: 8)
            entry.put(read == nil ? UInt32(1) : UInt32(0), at: 12)
        }

        seq.store(s + 2, ordering: .releasing)
    }

    /// The current `seq` (cheap: for watching for new frames).
    public static func currentSeq(_ base: UnsafeRawPointer) -> UInt64 {
        seqCell(base).load(ordering: .acquiring)
    }

    /// A consistent snapshot, or `nil` if the record is empty/foreign or kept changing.
    public static func read(_ base: UnsafeRawPointer) -> Snapshot? {
        let seq = seqCell(base)
        var copy = [UInt8](repeating: 0, count: size)
        for _ in 0..<64 {
            let s1 = seq.load(ordering: .acquiring)
            guard s1 != 0, s1 % 2 == 0 else {
                continue  // Swift has no spin-loop hint to put here
            }
            // Formally a data race, as in every other version of this code: a concurrent
            // write is detected by the seq check below.
            copy.withUnsafeMutableBytes { $0.baseAddress!.copyMemory(from: base, byteCount: size) }
            atomicMemoryFence(ordering: .acquiring)
            guard seq.load(ordering: .relaxed) == s1 else {
                continue
            }
            return parse(copy, seq: s1)
        }
        return nil
    }

    private static func parse(_ record: [UInt8], seq: UInt64) -> Snapshot? {
        record.withUnsafeBytes { (b: UnsafeRawBufferPointer) -> Snapshot? in
            func u32(_ offset: Int) -> UInt32 {
                UInt32(littleEndian: b.loadUnaligned(fromByteOffset: offset, as: UInt32.self))
            }
            func u64(_ offset: Int) -> UInt64 {
                UInt64(littleEndian: b.loadUnaligned(fromByteOffset: offset, as: UInt64.self))
            }

            guard b.prefix(8).elementsEqual(magic), u32(8) == version else {
                return nil
            }
            let count = min(Int(u32(12)), maxRegions)
            let info = FrameInfo(
                frame: u64(24),
                qpc: Int64(truncatingIfNeeded: u64(32)),
                qpcFrequency: Int64(truncatingIfNeeded: u64(40)),
                copyNs: u32(48),
                generation: u32(52)
            )

            var table: [MirrorRegion] = []
            table.reserveCapacity(count)
            for i in 0..<count {
                let entry = tableOffset + i * entrySize
                table.append(MirrorRegion(addr: u64(entry), len: u32(entry + 8)))
            }
            guard let offsets = try? layout(table) else {
                return nil
            }

            var regions: [RegionData] = []
            regions.reserveCapacity(count)
            for (i, (region, offset)) in zip(table, offsets).enumerated() {
                let bytes = Array(b[offset..<(offset + Int(region.len))])
                let ok = u32(tableOffset + i * entrySize + 12) == 0
                regions.append(RegionData(addr: region.addr, ok: ok, bytes: bytes))
            }
            return Snapshot(seq: seq, info: info, regions: regions)
        }
    }
}

extension UnsafeMutableRawPointer {
    fileprivate func put<T: FixedWidthInteger>(_ value: T, at offset: Int) {
        withUnsafeBytes(of: value.littleEndian) { bytes in
            (self + offset).copyMemory(from: bytes.baseAddress!, byteCount: bytes.count)
        }
    }
}
