//! Отображённые в память буферы игры и поиск в них камеры.
//!
//! Буферы констант игра пишет процессором в upload-буферы, отображённые через
//! `ID3D12Resource::Map`. Ресурсам-буферам подставляется копия vtable с перехватом
//! `Release` (2), `Map` (8), `Unmap` (9) — так известно, где лежат данные и когда
//! они перестают быть действительными. Поиск камеры — по раскладке из запроса:
//! положение камеры должно совпасть со столбцом переноса матрицы «вид → мир»,
//! проекция — выглядеть как перспективная. Это самопроверка, случайно не совпадёт.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use grace_proto::{CameraLayout, CameraReport, CameraSample};


use crate::debug;

// Слоты ID3D12Device* (d3d12.h): CreateCommittedResource 27, CreatePlacedResource 29,
// ID3D12Device4::CreateCommittedResource1 53, ID3D12Device8::CreateCommittedResource2 69,
// CreatePlacedResource1 70. ID3D12Resource: Release 2, Map 8, Unmap 9.
pub const SLOT_COMMITTED: usize = 27;
pub const SLOT_PLACED: usize = 29;
pub const SLOT_COMMITTED1: usize = 53;
pub const SLOT_COMMITTED2: usize = 69;
pub const SLOT_PLACED1: usize = 70;
const RES_RELEASE: usize = 2;
const RES_MAP: usize = 8;
const RES_UNMAP: usize = 9;
const RES_SLOTS: usize = 40;

/// IID_ID3D12Device8 {9218e6bb-f944-4f7e-a75c-b1b2c7b701f3}.
pub const IID_DEVICE8: [u8; 16] = crate::live::guid(0x9218e6bb, 0xf944, 0x4f7e, [0xa7, 0x5c, 0xb1, 0xb2, 0xc7, 0xb7, 0x01, 0xf3]);

/// Размер буфера по ресурсу (только D3D12_RESOURCE_DIMENSION_BUFFER).
static WIDTHS: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);
/// Отображённые буферы: ресурс → (указатель, длина).
static MAPPED: Mutex<Option<HashMap<usize, (usize, usize)>>> = Mutex::new(None);
/// Исходные vtable ресурсов и наши копии (на индексе −1 — адрес исходной).
static VTBLS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Оригиналы методов устройства (записываются при установке хуков).
pub static ORIG_COMMITTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static ORIG_PLACED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static ORIG_COMMITTED1: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static ORIG_COMMITTED2: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static ORIG_PLACED1: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

use std::sync::atomic::Ordering::Acquire;

/// D3D12_RESOURCE_DESC / DESC1: u32 Dimension (1 — буфер), u64 Alignment, u64 Width.
///
/// # Safety
/// `desc` — описание ресурса от игры или null.
unsafe fn buffer_width(desc: *const u8) -> Option<usize> {
    if desc.is_null() {
        return None;
    }
    // SAFETY: по контракту; поля по документированным смещениям.
    unsafe { (std::ptr::read_unaligned(desc as *const u32) == 1).then(|| std::ptr::read_unaligned(desc.add(16) as *const u64) as usize) }
}

/// После успешного создания: буферам — своя vtable и учёт размера.
///
/// # Safety
/// `out` — выходной указатель успешного вызова создания ресурса, `desc` — его описание.
unsafe fn created(hr: i32, desc: *const u8, out: *mut *mut c_void) -> i32 {
    if hr >= 0
        && !out.is_null()
        && let Some(width) = unsafe { buffer_width(desc) }
    {
        // SAFETY: при успехе *out — ресурс.
        let res = unsafe { *out };
        if !res.is_null() {
            if let Ok(mut w) = WIDTHS.lock() {
                w.get_or_insert_with(HashMap::new).insert(res as usize, width);
            }
            unsafe { adopt(res) };
        }
    }
    hr
}

unsafe fn adopt(res: *mut c_void) {
    let Ok(mut known) = VTBLS.lock() else { return };
    // SAFETY: первое поле COM-объекта — указатель на vtable.
    let vtbl = unsafe { *(res as *const usize) };
    if known.iter().any(|&(_, c)| c == vtbl) {
        return;
    }
    let clone = match known.iter().find(|&&(o, _)| o == vtbl) {
        Some(&(_, c)) => c,
        None => {
            let mut copy: Box<[usize; RES_SLOTS + 1]> = Box::new([0; RES_SLOTS + 1]);
            copy[0] = vtbl;
            // SAFETY: читаем RES_SLOTS указателей vtable (константные данные D3DMetal).
            unsafe { std::ptr::copy_nonoverlapping(vtbl as *const usize, copy.as_mut_ptr().add(1), RES_SLOTS) };
            copy[1 + RES_RELEASE] = res_release as *const () as usize;
            copy[1 + RES_MAP] = res_map as *const () as usize;
            copy[1 + RES_UNMAP] = res_unmap as *const () as usize;
            let c = Box::leak(copy).as_ptr() as usize + 8;
            known.push((vtbl, c));
            debug(&format!("буферам подставлена своя vtable (вариант {})", known.len()));
            c
        }
    };
    // SAFETY: объект в куче, первое поле можно переписать.
    unsafe { std::ptr::write_volatile(res as *mut usize, clone) };
}

/// # Safety
/// `this` — объект с нашей копией vtable.
unsafe fn original(this: *mut c_void, slot: usize) -> usize {
    // SAFETY: у копии на индексе −1 — адрес исходной vtable.
    unsafe { *((*(*(this as *const *const usize)).sub(1)) as *const usize).add(slot) }
}

fn forget(res: usize) {
    if let Ok(mut m) = MAPPED.lock()
        && let Some(m) = m.as_mut()
    {
        m.remove(&res);
    }
}

unsafe extern "system" fn res_release(this: *mut c_void) -> u32 {
    type F = unsafe extern "system" fn(*mut c_void) -> u32;
    // SAFETY: оригинал из исходной vtable.
    let f: F = unsafe { std::mem::transmute(original(this, RES_RELEASE)) };
    let n = unsafe { f(this) };
    if n == 0 {
        forget(this as usize);
        if let Ok(mut w) = WIDTHS.lock()
            && let Some(w) = w.as_mut()
        {
            w.remove(&(this as usize));
        }
    }
    n
}

unsafe extern "system" fn res_map(this: *mut c_void, sub: u32, range: *const u8, out: *mut *mut c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
    // SAFETY: оригинал из исходной vtable.
    let f: F = unsafe { std::mem::transmute(original(this, RES_MAP)) };
    let hr = unsafe { f(this, sub, range, out) };
    if hr >= 0 && sub == 0 && !out.is_null() {
        // SAFETY: при успехе *out — указатель на данные ресурса.
        let ptr = unsafe { *out } as usize;
        let width = WIDTHS.lock().ok().and_then(|w| w.as_ref().and_then(|w| w.get(&(this as usize)).copied()));
        if ptr != 0
            && let Some(width) = width
            && let Ok(mut m) = MAPPED.lock()
        {
            m.get_or_insert_with(HashMap::new).insert(this as usize, (ptr, width));
        }
    }
    hr
}

unsafe extern "system" fn res_unmap(this: *mut c_void, sub: u32, range: *const u8) {
    type F = unsafe extern "system" fn(*mut c_void, u32, *const u8);
    forget(this as usize);
    // SAFETY: оригинал из исходной vtable.
    let f: F = unsafe { std::mem::transmute(original(this, RES_UNMAP)) };
    unsafe { f(this, sub, range) }
}

// ---- хуки устройства ------------------------------------------------------------------------

type Riid = *const [u8; 16];
type Out = *mut *mut c_void;

pub unsafe extern "system" fn hooked_committed(this: *mut c_void, props: *const u8, flags: u32, desc: *const u8, state: u32, clear: *const u8, riid: Riid, out: Out) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *const u8, u32, *const u8, u32, *const u8, Riid, Out) -> i32;
    // SAFETY: оригинал записан при установке хука.
    let f: F = unsafe { std::mem::transmute(ORIG_COMMITTED.load(Acquire)) };
    unsafe { created(f(this, props, flags, desc, state, clear, riid, out), desc, out) }
}

pub unsafe extern "system" fn hooked_placed(this: *mut c_void, heap: *mut c_void, offset: u64, desc: *const u8, state: u32, clear: *const u8, riid: Riid, out: Out) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *const u8, u32, *const u8, Riid, Out) -> i32;
    // SAFETY: оригинал записан при установке хука.
    let f: F = unsafe { std::mem::transmute(ORIG_PLACED.load(Acquire)) };
    unsafe { created(f(this, heap, offset, desc, state, clear, riid, out), desc, out) }
}

pub unsafe extern "system" fn hooked_committed1(
    this: *mut c_void,
    props: *const u8,
    flags: u32,
    desc: *const u8,
    state: u32,
    clear: *const u8,
    session: *mut c_void,
    riid: Riid,
    out: Out,
) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *const u8, u32, *const u8, u32, *const u8, *mut c_void, Riid, Out) -> i32;
    // SAFETY: оригинал записан при установке хука.
    let f: F = unsafe { std::mem::transmute(ORIG_COMMITTED1.load(Acquire)) };
    unsafe { created(f(this, props, flags, desc, state, clear, session, riid, out), desc, out) }
}

pub unsafe extern "system" fn hooked_committed2(
    this: *mut c_void,
    props: *const u8,
    flags: u32,
    desc: *const u8,
    state: u32,
    clear: *const u8,
    session: *mut c_void,
    riid: Riid,
    out: Out,
) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *const u8, u32, *const u8, u32, *const u8, *mut c_void, Riid, Out) -> i32;
    // SAFETY: оригинал записан при установке хука.
    let f: F = unsafe { std::mem::transmute(ORIG_COMMITTED2.load(Acquire)) };
    unsafe { created(f(this, props, flags, desc, state, clear, session, riid, out), desc, out) }
}

pub unsafe extern "system" fn hooked_placed1(this: *mut c_void, heap: *mut c_void, offset: u64, desc: *const u8, state: u32, clear: *const u8, riid: Riid, out: Out) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void, u64, *const u8, u32, *const u8, Riid, Out) -> i32;
    // SAFETY: оригинал записан при установке хука.
    let f: F = unsafe { std::mem::transmute(ORIG_PLACED1.load(Acquire)) };
    unsafe { created(f(this, heap, offset, desc, state, clear, riid, out), desc, out) }
}

// ---- поиск камеры --------------------------------------------------------------------------

pub fn find_camera(layout: &CameraLayout) -> CameraReport {
    let mapped: Vec<(usize, usize)> = MAPPED.lock().ok().and_then(|m| m.as_ref().map(|m| m.values().copied().collect())).unwrap_or_default();
    let need = [layout.position as usize + 12, layout.inv_view as usize + 64, layout.proj as usize + 64, layout.screen.map_or(0, |s| s as usize + 16)].into_iter().max().unwrap_or(0);
    let align = (layout.align as usize).max(4);
    let mut candidates: Vec<CameraSample> = Vec::new();
    let mut bytes = 0u64;
    for &(ptr, len) in &mapped {
        bytes += len as u64;
        if len < need {
            continue;
        }
        let mut off = 0;
        while off + need <= len {
            let base = ptr + off;
            // SAFETY: буфер отображён (запись удаляется на Unmap/Release), читаем внутри его длины.
            let f = |o: usize| unsafe { std::ptr::read_unaligned((base + o) as *const f32) };
            if let Some(c) = grace_proto::camera_at(layout, &f)
                && !candidates.iter().any(|k| k.position == c.position && k.inv_view == c.inv_view)
            {
                candidates.push(c);
                if candidates.len() >= 16 {
                    break;
                }
            }
            off += align;
        }
    }
    CameraReport { buffers: mapped.len(), bytes, candidates }
}

