//! Поиск камеры в памяти процесса игры.
//!
//! Буфер констант камеры игра собирает в своей памяти и копирует в upload-буфер GPU.
//! Подменять vtable ресурсов D3DMetal нельзя (он опознаёт свои ресурсы по ней — игра
//! падает), поэтому по запросу обходим доступные на запись участки памяти процесса
//! (`VirtualQuery`) и читаем их копией (`ReadProcessMemory` — исчезнувший участок даёт
//! ошибку, а не падение). Кандидат проверяется самопроверяемой сигнатурой
//! (`grace_proto::camera_at`): положение камеры = столбец переноса «вид → мир»,
//! проекция перспективная.

use std::ffi::c_void;

use grace_proto::{CameraLayout, CameraReport, CameraSample};

#[repr(C)]
#[derive(Default)]
struct MemoryBasicInformation {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition: u16,
    _pad: u16,
    size: usize,
    state: u32,
    protect: u32,
    kind: u32,
    _pad2: u32,
}

unsafe extern "system" {
    fn VirtualQuery(addr: *const c_void, info: *mut MemoryBasicInformation, len: usize) -> usize;
    fn ReadProcessMemory(process: *mut c_void, base: *const c_void, buf: *mut c_void, size: usize, read: *mut usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
}

const MEM_COMMIT: u32 = 0x1000;
const WRITABLE: u32 = 0x04 | 0x08 | 0x40 | 0x80; // READWRITE, WRITECOPY, EXECUTE_READWRITE/WRITECOPY
const PAGE_GUARD: u32 = 0x100;
const CHUNK: usize = 8 << 20;
/// Шаг поиска в памяти процесса: копия cbuffer'а на стороне CPU выровнена не на 256.
const STEP: usize = 16;
const MAX_CANDIDATES: usize = 32;

/// Обход доступной на запись памяти процесса кусками; `visit(адрес, f)` вызывается с шагом
/// `step` и читает f32 по смещению от адреса; вернуть `false` — остановить обход.
/// Возвращает (участков, байт).
fn scan(need: usize, step: usize, mut visit: impl FnMut(usize, &dyn Fn(usize) -> f32) -> bool) -> (usize, u64) {
    let (mut regions, mut bytes) = (0usize, 0u64);
    let mut buf = vec![0u8; CHUNK + need];
    // SAFETY: псевдо-хендл текущего процесса.
    let process = unsafe { GetCurrentProcess() };
    let mut addr: usize = 0x10000;
    while addr < 0x7fff_ffff_0000 {
        let mut info = MemoryBasicInformation::default();
        // SAFETY: info — буфер нужного размера.
        if unsafe { VirtualQuery(addr as *const c_void, &mut info, size_of::<MemoryBasicInformation>()) } == 0 || info.size == 0 {
            break;
        }
        let end = info.base + info.size;
        if info.state == MEM_COMMIT && info.protect & WRITABLE != 0 && info.protect & PAGE_GUARD == 0 {
            regions += 1;
            bytes += info.size as u64;
            let mut at = info.base;
            while at + need <= end {
                let len = (end - at).min(CHUNK + need);
                let mut got = 0usize;
                // SAFETY: читаем копией в свой буфер; недоступная память даёт ошибку, а не падение.
                let ok = unsafe { ReadProcessMemory(process, at as *const c_void, buf.as_mut_ptr().cast(), len, &mut got) } != 0;
                if !ok || got < need {
                    break;
                }
                let data = &buf[..got];
                let mut off = 0;
                while off + need <= data.len() {
                    let f = |o: usize| f32::from_le_bytes([data[off + o], data[off + o + 1], data[off + o + 2], data[off + o + 3]]);
                    if !visit(at + off, &f) {
                        return (regions, bytes);
                    }
                    off += step;
                }
                // Следующий кусок перекрывает хвост на `need`, чтобы не пропустить структуру на стыке.
                at += got.saturating_sub(need).max(step);
            }
        }
        addr = end;
    }
    (regions, bytes)
}

pub fn find_camera(layout: &CameraLayout) -> CameraReport {
    let need = [layout.position as usize + 12, layout.inv_view as usize + 64, layout.proj as usize + 64, layout.screen.map_or(0, |s| s as usize + 16)].into_iter().max().unwrap_or(64);
    let mut candidates: Vec<CameraSample> = Vec::new();
    let mut projections: Vec<(u64, f32, f32)> = Vec::new();
    let (regions, bytes) = scan(need, STEP, |addr, f| {
        if let Some(c) = grace_proto::camera_at(layout, f)
            && !candidates.iter().any(|k| k.position == c.position && k.inv_view == c.inv_view)
        {
            candidates.push(c);
        }
        if projections.len() < 64
            && let Some((fovy, aspect)) = grace_proto::projection_at(f)
        {
            projections.push((addr as u64, fovy, aspect));
        }
        candidates.len() < MAX_CANDIDATES
    });
    // Второй проход: та же ориентация камеры, другой перенос — «мировая» копия.
    let mut same_rotation: Vec<(u64, [f32; 3])> = Vec::new();
    if let Some(c) = candidates.first() {
        let rot: [f32; 9] = std::array::from_fn(|k| c.inv_view[(k / 3) * 4 + k % 3]);
        let local = c.position;
        scan(64, 4, |addr, f| {
            if let Some(t) = grace_proto::rotation_match(&rot, f) {
                let differs = (0..3).any(|a| (t[a] - local[a]).abs() > 0.5);
                if differs && !same_rotation.iter().any(|(_, k)| *k == t) {
                    same_rotation.push((addr as u64, t));
                }
            }
            same_rotation.len() < 64
        });
    }
    CameraReport { buffers: regions, bytes, candidates, projections, same_rotation }
}

/// Адреса подряд идущих f32 в диапазонах (шаг 4 байта), не больше `limit`.
pub fn find_floats(ranges: &[(f32, f32)], limit: u32) -> (usize, Vec<u64>) {
    let mut addrs = Vec::new();
    if ranges.is_empty() {
        return (0, addrs);
    }
    let (regions, _) = scan(ranges.len() * 4, 4, |addr, f| {
        if ranges.iter().enumerate().all(|(i, &(lo, hi))| {
            let v = f(i * 4);
            v >= lo && v <= hi
        }) {
            addrs.push(addr as u64);
        }
        addrs.len() < limit as usize
    });
    (regions, addrs)
}

/// Отладочное чтение памяти игры (ошибка чтения — пустой ответ, не падение).
pub fn peek(addr: u64, len: u32) -> Vec<u8> {
    let mut buf = vec![0u8; (len as usize).min(1 << 20)];
    let mut got = 0usize;
    // SAFETY: читаем копией в свой буфер через ReadProcessMemory.
    let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), addr as usize as *const c_void, buf.as_mut_ptr().cast(), buf.len(), &mut got) } != 0;
    buf.truncate(if ok { got } else { 0 });
    buf
}
