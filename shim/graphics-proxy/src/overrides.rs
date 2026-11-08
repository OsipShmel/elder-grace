//! Shader overrides (phase C1).
//!
//! The `GRACE_SHADER_OVERRIDE` directory holds `<fnv64>.<stage>.dxbc` files; the hash is the same
//! as in the dump (`fnv1a` of the original bytecode). When the game creates a pipeline with such a shader,
//! D3DMetal gets a copy of the pipeline description where that stage's bytecode is replaced by ours.
//! The root signature and the rest of the state stay the game's, so the replacement must
//! declare the same resources and the same input/output signature.
//!
//! The shim rejects loading an overridden pipeline from the library (the game's cache) as a miss:
//! the game then creates it through the device, where the override applies.
//!
//! The game always gets the original PSO; the override is created second and substituted
//! into command lists (`live.rs`) while overrides are on.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use crate::{debug, fnv1a, subobject_layout};

/// sizeof(D3D12_GRAPHICS_PIPELINE_STATE_DESC) on x64.
pub const GRAPHICS_DESC_SIZE: usize = 656;
/// sizeof(D3D12_COMPUTE_PIPELINE_STATE_DESC) on x64.
pub const COMPUTE_DESC_SIZE: usize = 56;

type Table = Arc<HashMap<u64, &'static [u8]>>;

static TABLE: RwLock<Option<Table>> = RwLock::new(None);

/// Reads the `GRACE_SHADER_OVERRIDE` directory. Bytes of previous overrides are never freed:
/// PSOs already created and descriptions in flight may point to them.
fn load_dir() -> HashMap<u64, &'static [u8]> {
    let mut map = HashMap::new();
    let Some(dir) = std::env::var_os("GRACE_SHADER_OVERRIDE") else { return map };
    let Ok(entries) = std::fs::read_dir(&dir) else { return map };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(hex) = name.split('.').next() else { continue };
        let (Ok(hash), true) = (u64::from_str_radix(hex, 16), name.ends_with(".dxbc")) else { continue };
        if let Ok(bytes) = std::fs::read(&path) {
            map.insert(hash, &*Box::leak(bytes.into_boxed_slice()));
        }
    }
    debug(&format!("shader overrides loaded: {}", map.len()));
    map
}

fn table() -> Table {
    if let Ok(t) = TABLE.read()
        && let Some(t) = t.as_ref()
    {
        return t.clone();
    }
    let Ok(mut t) = TABLE.write() else { return Arc::default() };
    t.get_or_insert_with(|| Arc::new(load_dir())).clone()
}

/// Re-reads the override directory; returns how many were loaded.
pub fn reload() -> usize {
    let fresh = Arc::new(load_dir());
    let n = fresh.len();
    if let Ok(mut t) = TABLE.write() {
        *t = Some(fresh);
    }
    n
}

/// Override bytecode for the original shader with hash `hash`.
pub fn get(hash: u64) -> Option<&'static [u8]> {
    table().get(&hash).copied()
}

/// How many override files are loaded.
pub fn loaded() -> usize {
    table().len()
}

/// Offset of D3D12_CACHED_PIPELINE_STATE in D3D12_GRAPHICS_PIPELINE_STATE_DESC (x64).
const GRAPHICS_CACHED_PSO: usize = 632;
/// The same in D3D12_COMPUTE_PIPELINE_STATE_DESC.
const COMPUTE_CACHED_PSO: usize = 32;
/// Subobject type D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_CACHED_PSO.
const SUBOBJECT_CACHED_PSO: u32 = 19;

/// The driver cache was built for the original shaders and is useless in an overridden description.
fn drop_cached_pso(copy: &mut [u8], off: usize) {
    copy[off..off + 16].fill(0);
}

/// Override for the D3D12_SHADER_BYTECODE at address `bc`, if any.
///
/// # Safety
/// `bc` points to D3D12_SHADER_BYTECODE { const void*, SIZE_T }.
unsafe fn lookup(bc: *const u8) -> Option<(u64, &'static [u8])> {
    let t = table();
    if t.is_empty() {
        return None;
    }
    // SAFETY: per the function contract.
    let (ptr, len) = unsafe { (*(bc as *const *const u8), *(bc.add(8) as *const usize)) };
    if ptr.is_null() || len == 0 || len > 64 << 20 {
        return None;
    }
    // SAFETY: bytecode of length len from the game.
    let hash = fnv1a(unsafe { std::slice::from_raw_parts(ptr, len) });
    t.get(&hash).map(|b| (hash, *b))
}

fn note_applied(hash: u64, stage: &str) {
    static SEEN: Mutex<Option<HashSet<u64>>> = Mutex::new(None);
    if let Ok(mut s) = SEEN.lock()
        && s.get_or_insert_with(HashSet::new).insert(hash)
    {
        debug(&format!("shader override {hash:016x}.{stage} applied"));
        crate::live::note_applied(format!("{hash:016x}.{stage}"));
    }
}

/// Writes a new D3D12_SHADER_BYTECODE into the description copy.
fn write_bytecode(copy: &mut [u8], off: usize, bytes: &'static [u8]) {
    copy[off..off + 8].copy_from_slice(&(bytes.as_ptr() as usize).to_le_bytes());
    copy[off + 8..off + 16].copy_from_slice(&bytes.len().to_le_bytes());
}

const GRAPHICS_STAGES: [&str; 5] = ["vs", "ps", "ds", "hs", "gs"];

/// Whether at least one stage of a graphics description has an override.
///
/// # Safety
/// `desc` — D3D12_GRAPHICS_PIPELINE_STATE_DESC.
pub unsafe fn graphics_has_override(desc: *const u8) -> bool {
    (0..GRAPHICS_STAGES.len()).any(|i| unsafe { lookup(desc.add(8 + i * 16)) }.is_some())
}

/// Copy of a graphics description with overridden stages (or None if there are no overrides).
///
/// # Safety
/// `desc` is a D3D12_GRAPHICS_PIPELINE_STATE_DESC (656 bytes).
pub unsafe fn graphics_copy(desc: *const u8) -> Option<Box<[u8; GRAPHICS_DESC_SIZE]>> {
    let mut copy: Option<Box<[u8; GRAPHICS_DESC_SIZE]>> = None;
    for (i, stage) in GRAPHICS_STAGES.iter().enumerate() {
        let off = 8 + i * 16;
        if let Some((hash, bytes)) = unsafe { lookup(desc.add(off)) } {
            let c = copy.get_or_insert_with(|| {
                let mut b = Box::new([0u8; GRAPHICS_DESC_SIZE]);
                // SAFETY: the game's description has size GRAPHICS_DESC_SIZE.
                unsafe { std::ptr::copy_nonoverlapping(desc, b.as_mut_ptr(), GRAPHICS_DESC_SIZE) };
                b
            });
            write_bytecode(&mut c[..], off, bytes);
            note_applied(hash, stage);
        }
    }
    if let Some(c) = copy.as_mut() {
        drop_cached_pso(&mut c[..], GRAPHICS_CACHED_PSO);
    }
    copy
}

/// # Safety
/// `desc` — D3D12_COMPUTE_PIPELINE_STATE_DESC.
pub unsafe fn compute_has_override(desc: *const u8) -> bool {
    unsafe { lookup(desc.add(8)) }.is_some()
}

/// # Safety
/// `desc` is a D3D12_COMPUTE_PIPELINE_STATE_DESC (56 bytes).
pub unsafe fn compute_copy(desc: *const u8) -> Option<Box<[u8; COMPUTE_DESC_SIZE]>> {
    let (hash, bytes) = unsafe { lookup(desc.add(8)) }?;
    let mut b = Box::new([0u8; COMPUTE_DESC_SIZE]);
    // SAFETY: the game's description has size COMPUTE_DESC_SIZE.
    unsafe { std::ptr::copy_nonoverlapping(desc, b.as_mut_ptr(), COMPUTE_DESC_SIZE) };
    write_bytecode(&mut b[..], 8, bytes);
    drop_cached_pso(&mut b[..], COMPUTE_CACHED_PSO);
    note_applied(hash, "cs");
    Some(b)
}

fn stream_stage(ty: u32) -> Option<&'static str> {
    Some(match ty {
        1 => "vs",
        2 => "ps",
        3 => "ds",
        4 => "hs",
        5 => "gs",
        6 => "cs",
        24 => "as",
        25 => "ms",
        _ => return None,
    })
}

/// Copy of a subobject stream with overridden shaders (or None).
///
/// # Safety
/// `stream` is a PSO subobject stream of length `size`.
pub unsafe fn stream_copy(stream: *const u8, size: usize) -> Option<Vec<u8>> {
    if table().is_empty() {
        return None;
    }
    // SAFETY: a stream of length size from the game.
    let mut copy = unsafe { std::slice::from_raw_parts(stream, size) }.to_vec();
    let mut changed = false;
    let mut off = 0usize;
    while off + 4 <= size {
        let ty = u32::from_le_bytes([copy[off], copy[off + 1], copy[off + 2], copy[off + 3]]);
        let Some((data_off, data_size)) = subobject_layout(ty) else { break };
        if let Some(stage) = stream_stage(ty)
            && let Some((hash, bytes)) = unsafe { lookup(stream.add(off + data_off)) }
        {
            write_bytecode(&mut copy, off + data_off, bytes);
            note_applied(hash, stage);
            changed = true;
        }
        if ty == SUBOBJECT_CACHED_PSO {
            drop_cached_pso(&mut copy, off + data_off);
        }
        off += (data_off + data_size).next_multiple_of(8);
    }
    changed.then_some(copy)
}

/// Whether the subobject stream has an override.
///
/// # Safety
/// `stream` is a PSO subobject stream of length `size`.
pub unsafe fn stream_has_override(stream: *const u8, size: usize) -> bool {
    if table().is_empty() {
        return false;
    }
    let mut off = 0usize;
    while off + 4 <= size {
        // SAFETY: inside a stream of length size.
        let ty = unsafe { std::ptr::read_unaligned(stream.add(off) as *const u32) };
        let Some((data_off, data_size)) = subobject_layout(ty) else { return false };
        if stream_stage(ty).is_some() && unsafe { lookup(stream.add(off + data_off)) }.is_some() {
            return true;
        }
        off += (data_off + data_size).next_multiple_of(8);
    }
    false
}
