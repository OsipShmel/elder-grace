//! Live mirror writer: at every `Present` copies the requested regions of game memory into
//! the mapped mirror file (`GRACE_MIRROR`, a Windows path). Format: `grace_proto::mirror`.
//! `Present` never waits: if the region set is being replaced, that frame is skipped.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use grace_proto::mirror::{self, FrameInfo};
use grace_proto::MirrorRegion;

use crate::debug;

unsafe extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: *mut c_void) -> *mut c_void;
    fn CreateFileMappingW(file: *mut c_void, sa: *mut c_void, protect: u32, size_high: u32, size_low: u32, name: *const u16) -> *mut c_void;
    fn MapViewOfFile(mapping: *mut c_void, access: u32, offset_high: u32, offset_low: u32, size: usize) -> *mut c_void;
    fn ReadProcessMemory(process: *mut c_void, base: *const c_void, buf: *mut c_void, size: usize, read: *mut usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn QueryPerformanceCounter(out: *mut i64) -> i32;
    fn QueryPerformanceFrequency(out: *mut i64) -> i32;
}

const GENERIC_RW: u32 = 0x8000_0000 | 0x4000_0000;
const SHARE_RW: u32 = 0x1 | 0x2;
const OPEN_ALWAYS: u32 = 4;
const PAGE_READWRITE: u32 = 0x04;
const FILE_MAP_WRITE: u32 = 0x2;
const INVALID_HANDLE: isize = -1;

/// Base of the mapped record (0: mirroring is off).
static BASE: AtomicUsize = AtomicUsize::new(0);

struct Set {
    regions: Vec<MirrorRegion>,
    offsets: Vec<usize>,
    generation: u32,
}

static SET: Mutex<Set> = Mutex::new(Set { regions: Vec::new(), offsets: Vec::new(), generation: 0 });

/// Maps the mirror file named by `GRACE_MIRROR` (once, from the swapchain hook).
pub fn start() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    STARTED.get_or_init(map_file);
}

fn map_file() {
    let Some(path) = std::env::var_os("GRACE_MIRROR") else { return };
    let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
    // SAFETY: plain Win32 calls with a null-terminated path; handles stay open for the process lifetime.
    let base = unsafe {
        let file = CreateFileW(wide.as_ptr(), GENERIC_RW, SHARE_RW, std::ptr::null_mut(), OPEN_ALWAYS, 0, std::ptr::null_mut());
        if file as isize == INVALID_HANDLE {
            debug(&format!("mirror: cannot open {}", path.to_string_lossy()));
            return;
        }
        let mapping = CreateFileMappingW(file, std::ptr::null_mut(), PAGE_READWRITE, 0, mirror::SIZE as u32, std::ptr::null());
        if mapping.is_null() {
            debug("mirror: CreateFileMapping failed");
            return;
        }
        MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, mirror::SIZE)
    };
    if base.is_null() {
        debug("mirror: MapViewOfFile failed");
        return;
    }
    BASE.store(base as usize, Ordering::Release);
    debug(&format!("mirror: {}", path.to_string_lossy()));
}

/// Replaces the region set (control thread). Returns the new generation.
pub fn set_regions(regions: Vec<MirrorRegion>) -> Result<u32, String> {
    if BASE.load(Ordering::Acquire) == 0 {
        return Err("mirroring is off: start the game with GRACE_MIRROR".into());
    }
    let offsets = mirror::layout(&regions)?;
    let mut set = SET.lock().map_err(|_| "mirror state poisoned".to_owned())?;
    set.regions = regions;
    set.offsets = offsets;
    set.generation = set.generation.wrapping_add(1);
    Ok(set.generation)
}

/// Copies the regions into the record (render thread, at `Present`).
pub fn on_present(frame: u64) {
    let base = BASE.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    let Ok(set) = SET.try_lock() else { return };
    let mut t0 = 0i64;
    let mut freq = 0i64;
    // SAFETY: out-pointers to locals.
    unsafe {
        QueryPerformanceCounter(&mut t0);
        QueryPerformanceFrequency(&mut freq);
    }
    // SAFETY: GetCurrentProcess returns a pseudo-handle.
    let process = unsafe { GetCurrentProcess() };
    let copy = |i: usize, dst: &mut [u8]| {
        let mut got = 0usize;
        // SAFETY: ReadProcessMemory copies into our buffer and fails (no fault) on unmapped memory.
        let ok = unsafe { ReadProcessMemory(process, set.regions[i].addr as usize as *const c_void, dst.as_mut_ptr().cast(), dst.len(), &mut got) } != 0;
        ok && got == dst.len()
    };
    let mut info = FrameInfo { frame, qpc: t0, qpc_freq: freq, copy_ns: 0, generation: set.generation };
    // The copy time of this frame is known only after it, so the header carries the previous frame's.
    info.copy_ns = LAST_COPY_NS.load(Ordering::Relaxed) as u32;
    // SAFETY: `base` maps mirror::SIZE bytes (page-aligned) and this is the only writer.
    unsafe { mirror::write(base as *mut u8, &set.regions, &set.offsets, &info, copy) };
    let mut t1 = 0i64;
    // SAFETY: out-pointer to a local.
    unsafe { QueryPerformanceCounter(&mut t1) };
    if freq > 0 {
        LAST_COPY_NS.store(((t1 - t0) as i128 * 1_000_000_000 / freq as i128) as usize, Ordering::Relaxed);
    }
}

static LAST_COPY_NS: AtomicUsize = AtomicUsize::new(0);
