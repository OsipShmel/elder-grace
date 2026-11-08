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

pub fn find_camera(layout: &CameraLayout) -> CameraReport {
    let need = [layout.position as usize + 12, layout.inv_view as usize + 64, layout.proj as usize + 64, layout.screen.map_or(0, |s| s as usize + 16)].into_iter().max().unwrap_or(64);
    let mut candidates: Vec<CameraSample> = Vec::new();
    let (mut regions, mut bytes) = (0usize, 0u64);
    let mut buf = vec![0u8; CHUNK + need];
    // SAFETY: псевдо-хендл текущего процесса.
    let process = unsafe { GetCurrentProcess() };
    let mut addr: usize = 0x10000;
    while addr < 0x7fff_ffff_0000 && candidates.len() < MAX_CANDIDATES {
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
            while at + need <= end && candidates.len() < MAX_CANDIDATES {
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
                    if let Some(c) = grace_proto::camera_at(layout, &f)
                        && !candidates.iter().any(|k| k.position == c.position && k.inv_view == c.inv_view)
                    {
                        candidates.push(c);
                    }
                    off += STEP;
                }
                // Следующий кусок перекрывает хвост на `need`, чтобы не пропустить структуру на стыке.
                at += got.saturating_sub(need).max(STEP);
            }
        }
        addr = end;
    }
    CameraReport { buffers: regions, bytes, candidates }
}
