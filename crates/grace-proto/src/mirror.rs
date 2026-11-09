//! The live mirror record: a fixed 64 KiB block shared between the shim (writer, inside the
//! game process) and a native reader through a mapped file. A seqlock in the header gives
//! consistent snapshots: the writer makes `seq` odd while it writes, the reader retries on
//! an odd or changed `seq`. See ADR 0006.

use std::sync::atomic::{AtomicU64, Ordering, fence};

use serde::{Deserialize, Serialize};

pub const MAGIC: [u8; 8] = *b"GRMIRROR";
pub const VERSION: u32 = 1;
/// Size of the record (and of the mirror file).
pub const SIZE: usize = 64 * 1024;
pub const MAX_REGIONS: usize = 64;

const SEQ: usize = 16;
const TABLE: usize = 64;
const ENTRY: usize = 16;
const DATA: usize = TABLE + MAX_REGIONS * ENTRY;

/// A memory region of the game process to copy every frame.
///
/// Without `chain` the region is at `addr`. With a chain the shim follows pointers every frame:
/// `p = addr; for off in chain { p = *(p as *const u64) + off }`, so `addr` is where the first
/// pointer lives (for example a static in the game's image) and the last offset points at the
/// data. The record's table holds the resolved address.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MirrorRegion {
    pub addr: u64,
    pub len: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<i64>,
}

impl MirrorRegion {
    pub fn at(addr: u64, len: u32) -> Self {
        Self { addr, len, chain: Vec::new() }
    }
}

/// Per-frame header values written by the shim.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameInfo {
    /// `Present` count.
    pub frame: u64,
    /// QueryPerformanceCounter at the copy and its frequency.
    pub qpc: i64,
    pub qpc_freq: i64,
    /// Time the shim spent copying, ns.
    pub copy_ns: u32,
    /// Bumps every time the region set changes.
    pub generation: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionData {
    pub addr: u64,
    /// False when the shim could not read the region (freed or unmapped memory).
    pub ok: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub seq: u64,
    pub info: FrameInfo,
    pub regions: Vec<RegionData>,
}

/// Data offsets of the regions inside the record, or an error if they do not fit.
pub fn layout(regions: &[MirrorRegion]) -> Result<Vec<usize>, String> {
    if regions.len() > MAX_REGIONS {
        return Err(format!("at most {MAX_REGIONS} regions, got {}", regions.len()));
    }
    let mut at = DATA;
    let mut offsets = Vec::with_capacity(regions.len());
    for r in regions {
        offsets.push(at);
        at = (at + r.len as usize).next_multiple_of(16);
    }
    if at > SIZE {
        return Err(format!("regions need {} bytes, the record holds {}", at - DATA, SIZE - DATA));
    }
    Ok(offsets)
}

fn seq_of(base: *const u8) -> &'static AtomicU64 {
    // SAFETY: callers guarantee `base` is 8-aligned and points to SIZE bytes that outlive the use.
    unsafe { AtomicU64::from_ptr(base.add(SEQ) as *mut u64) }
}

/// Writes one frame. `offsets` comes from [`layout`] for the same `regions`; `fill(i, dst)`
/// copies region `i` into `dst` and returns the address it read from (`None` if the read failed).
///
/// # Safety
/// `base` is 8-aligned, points to [`SIZE`] writable bytes, and there is a single writer.
pub unsafe fn write(base: *mut u8, regions: &[MirrorRegion],
                    offsets: &[usize], info: &FrameInfo,
                    mut fill: impl FnMut(usize, &mut [u8]) -> Option<u64>) {
    let seq = seq_of(base);
    let s = seq.load(Ordering::Relaxed);
    let s = if s % 2 == 1 { s + 1 } else { s };
    seq.store(s + 1, Ordering::Relaxed);
    fence(Ordering::Release);
    // SAFETY: all offsets are inside SIZE (checked by `layout`); the reader tolerates torn bytes.
    unsafe {
        let put = |off: usize, bytes: &[u8]| std::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(off), bytes.len());
        put(0, &MAGIC);
        put(8, &VERSION.to_le_bytes());
        put(12, &(regions.len() as u32).to_le_bytes());
        put(24, &info.frame.to_le_bytes());
        put(32, &info.qpc.to_le_bytes());
        put(40, &info.qpc_freq.to_le_bytes());
        put(48, &info.copy_ns.to_le_bytes());
        put(52, &info.generation.to_le_bytes());
        for (i, (r, &off)) in regions.iter().zip(offsets).enumerate() {
            let dst = std::slice::from_raw_parts_mut(base.add(off), r.len as usize);
            let read = fill(i, dst);
            let e = TABLE + i * ENTRY;
            put(e, &read.unwrap_or(r.addr).to_le_bytes());
            put(e + 8, &r.len.to_le_bytes());
            put(e + 12, &u32::from(read.is_none()).to_le_bytes());
        }
    }
    seq.store(s + 2, Ordering::Release);
}

/// The current `seq` (cheap: for watching for new frames).
///
/// # Safety
/// `base` is 8-aligned and points to [`SIZE`] readable bytes.
pub unsafe fn current_seq(base: *const u8) -> u64 {
    seq_of(base).load(Ordering::Acquire)
}

/// A consistent snapshot, or `None` if the record is empty/foreign or kept changing.
///
/// # Safety
/// `base` is 8-aligned and points to [`SIZE`] readable bytes.
pub unsafe fn read(base: *const u8) -> Option<Snapshot> {
    let seq = seq_of(base);
    for _ in 0..64 {
        let s1 = seq.load(Ordering::Acquire);
        if s1 == 0 || s1 % 2 == 1 {
            std::hint::spin_loop();
            continue;
        }
        let mut copy = vec![0u8; SIZE];
        // SAFETY: SIZE readable bytes; a concurrent write is detected by the seq check below.
        unsafe { std::ptr::copy_nonoverlapping(base, copy.as_mut_ptr(), SIZE) };
        fence(Ordering::Acquire);
        if seq.load(Ordering::Relaxed) != s1 {
            continue;
        }
        return parse(&copy, s1);
    }
    None
}

fn parse(b: &[u8], seq: u64) -> Option<Snapshot> {
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    if b[..8] != MAGIC || u32_at(8) != VERSION {
        return None;
    }
    let n = (u32_at(12) as usize).min(MAX_REGIONS);
    let info = FrameInfo { frame: u64_at(24), qpc: u64_at(32) as i64, qpc_freq: u64_at(40) as i64, copy_ns: u32_at(48), generation: u32_at(52) };
    let table: Vec<MirrorRegion> = (0..n).map(|i| MirrorRegion::at(u64_at(TABLE + i * ENTRY), u32_at(TABLE + i * ENTRY + 8))).collect();
    let offsets = layout(&table).ok()?;
    let regions = table
        .iter()
        .zip(offsets)
        .enumerate()
        .map(|(i, (r, off))| RegionData { addr: r.addr, ok: u32_at(TABLE + i * ENTRY + 12) == 0, bytes: b[off..off + r.len as usize].to_vec() })
        .collect();
    Some(Snapshot { seq, info, regions })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Vec<u64> {
        vec![0u64; SIZE / 8]
    }

    #[test]
    fn roundtrip() {
        let mut buf = record();
        let base = buf.as_mut_ptr().cast::<u8>();
        assert_eq!(unsafe { read(base) }, None, "an empty record has no snapshot");
        let regions = [MirrorRegion::at(0x1000, 5), MirrorRegion { addr: 0x2000, len: 3, chain: vec![8] }];
        let offsets = layout(&regions).unwrap();
        let info = FrameInfo { frame: 7, qpc: 123, qpc_freq: 10_000_000, copy_ns: 900, generation: 2 };
        unsafe {
            write(base, &regions, &offsets, &info, |i, dst| {
                dst.fill(i as u8 + 1);
                (i == 0).then_some(0x1000)
            })
        };
        let s = unsafe { read(base) }.unwrap();
        assert_eq!(s.seq, 2);
        assert_eq!(s.info, info);
        assert_eq!(s.regions[0], RegionData { addr: 0x1000, ok: true, bytes: vec![1; 5] });
        assert_eq!(s.regions[1], RegionData { addr: 0x2000, ok: false, bytes: vec![2; 3] });
        unsafe { write(base, &regions, &offsets, &info, |_, _| Some(0x1000)) };
        assert_eq!(unsafe { current_seq(base) }, 4);
    }

    #[test]
    fn torn_record_is_not_returned() {
        let mut buf = record();
        let base = buf.as_mut_ptr().cast::<u8>();
        unsafe { write(base, &[], &[], &FrameInfo::default(), |_, _| None) };
        buf[SEQ / 8] = 3; // writer in the middle of a frame
        assert_eq!(unsafe { read(buf.as_ptr().cast()) }, None);
    }

    #[test]
    fn limits() {
        assert!(layout(&vec![MirrorRegion::at(0, 1); MAX_REGIONS + 1]).is_err());
        assert!(layout(&[MirrorRegion::at(0, (SIZE - DATA) as u32)]).is_ok());
        assert!(layout(&[MirrorRegion::at(0, (SIZE - DATA + 1) as u32)]).is_err());
    }

    #[test]
    fn concurrent_reads_are_consistent() {
        let mut buf = record();
        let addr = buf.as_mut_ptr() as usize;
        let regions = vec![MirrorRegion::at(0, 4096)];
        let offsets = layout(&regions).unwrap();
        let writer = std::thread::spawn(move || {
            for f in 1..=20_000u64 {
                let info = FrameInfo { frame: f, ..Default::default() };
                unsafe { write(addr as *mut u8, &regions, &offsets, &info, |_, dst| { dst.fill(f as u8); Some(0) }) };
            }
        });
        let mut seen = 0;
        while !writer.is_finished() {
            if let Some(s) = unsafe { read(addr as *const u8) } {
                let b = &s.regions[0].bytes;
                assert!(b.iter().all(|&x| x == s.info.frame as u8), "torn snapshot at frame {}", s.info.frame);
                seen += 1;
            }
        }
        writer.join().unwrap();
        assert!(seen > 0);
        drop(buf);
    }
}
