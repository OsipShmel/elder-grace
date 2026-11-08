//! Подмена шейдеров (фаза C1).
//!
//! Каталог `GRACE_SHADER_OVERRIDE` содержит файлы `<fnv64>.<стадия>.dxbc` — хеш тот же,
//! что у дампа (`fnv1a` исходного байткода). Когда игра создаёт пайплайн с таким шейдером,
//! D3DMetal получает копию описания пайплайна, где байткод этой стадии заменён нашим.
//! Корневая сигнатура и остальное состояние остаются игровыми, поэтому замена должна
//! объявлять те же ресурсы и ту же сигнатуру входов/выходов.
//!
//! Загрузку подменённого пайплайна из библиотеки (кэша игры) shim отклоняет как промах:
//! игра тогда создаёт его через устройство, а там подмена уже срабатывает.
//!
//! Игра всегда получает оригинальный PSO; подменённый создаётся вторым и подставляется
//! в командные списки (`live.rs`), пока подмены включены.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use crate::{debug, fnv1a, subobject_layout};

/// sizeof(D3D12_GRAPHICS_PIPELINE_STATE_DESC) на x64.
pub const GRAPHICS_DESC_SIZE: usize = 656;
/// sizeof(D3D12_COMPUTE_PIPELINE_STATE_DESC) на x64.
pub const COMPUTE_DESC_SIZE: usize = 56;

type Table = Arc<HashMap<u64, &'static [u8]>>;

static TABLE: RwLock<Option<Table>> = RwLock::new(None);

/// Читает каталог `GRACE_SHADER_OVERRIDE`. Байты прежних подмен не освобождаются:
/// на них могут ссылаться уже созданные PSO и описания в полёте.
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
    debug(&format!("подмен шейдеров загружено: {}", map.len()));
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

/// Перечитать каталог подмен; возвращает число загруженных.
pub fn reload() -> usize {
    let fresh = Arc::new(load_dir());
    let n = fresh.len();
    if let Ok(mut t) = TABLE.write() {
        *t = Some(fresh);
    }
    n
}

/// Байткод подмены для исходного шейдера с хешем `hash`.
pub fn get(hash: u64) -> Option<&'static [u8]> {
    table().get(&hash).copied()
}

/// Сколько файлов подмен загружено.
pub fn loaded() -> usize {
    table().len()
}

/// Смещение D3D12_CACHED_PIPELINE_STATE в D3D12_GRAPHICS_PIPELINE_STATE_DESC (x64).
const GRAPHICS_CACHED_PSO: usize = 632;
/// То же в D3D12_COMPUTE_PIPELINE_STATE_DESC.
const COMPUTE_CACHED_PSO: usize = 32;
/// Тип подобъекта D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_CACHED_PSO.
const SUBOBJECT_CACHED_PSO: u32 = 19;

/// Кэш драйвера собран для исходных шейдеров — в подменённом описании он не годится.
fn drop_cached_pso(copy: &mut [u8], off: usize) {
    copy[off..off + 16].fill(0);
}

/// Подмена для D3D12_SHADER_BYTECODE по адресу `bc`, если есть.
///
/// # Safety
/// `bc` указывает на D3D12_SHADER_BYTECODE { const void*, SIZE_T }.
unsafe fn lookup(bc: *const u8) -> Option<(u64, &'static [u8])> {
    let t = table();
    if t.is_empty() {
        return None;
    }
    // SAFETY: по контракту функции.
    let (ptr, len) = unsafe { (*(bc as *const *const u8), *(bc.add(8) as *const usize)) };
    if ptr.is_null() || len == 0 || len > 64 << 20 {
        return None;
    }
    // SAFETY: байткод длиной len от игры.
    let hash = fnv1a(unsafe { std::slice::from_raw_parts(ptr, len) });
    t.get(&hash).map(|b| (hash, *b))
}

fn note_applied(hash: u64, stage: &str) {
    static SEEN: Mutex<Option<HashSet<u64>>> = Mutex::new(None);
    if let Ok(mut s) = SEEN.lock()
        && s.get_or_insert_with(HashSet::new).insert(hash)
    {
        debug(&format!("подмена шейдера {hash:016x}.{stage} применена"));
        crate::live::note_applied(format!("{hash:016x}.{stage}"));
    }
}

/// Записывает новый D3D12_SHADER_BYTECODE в копию описания.
fn write_bytecode(copy: &mut [u8], off: usize, bytes: &'static [u8]) {
    copy[off..off + 8].copy_from_slice(&(bytes.as_ptr() as usize).to_le_bytes());
    copy[off + 8..off + 16].copy_from_slice(&bytes.len().to_le_bytes());
}

const GRAPHICS_STAGES: [&str; 5] = ["vs", "ps", "ds", "hs", "gs"];

/// Есть ли подмена хотя бы для одной стадии графического описания.
///
/// # Safety
/// `desc` — D3D12_GRAPHICS_PIPELINE_STATE_DESC.
pub unsafe fn graphics_has_override(desc: *const u8) -> bool {
    (0..GRAPHICS_STAGES.len()).any(|i| unsafe { lookup(desc.add(8 + i * 16)) }.is_some())
}

/// Копия графического описания с подменёнными стадиями (или None, если подмен нет).
///
/// # Safety
/// `desc` — D3D12_GRAPHICS_PIPELINE_STATE_DESC (656 байт).
pub unsafe fn graphics_copy(desc: *const u8) -> Option<Box<[u8; GRAPHICS_DESC_SIZE]>> {
    let mut copy: Option<Box<[u8; GRAPHICS_DESC_SIZE]>> = None;
    for (i, stage) in GRAPHICS_STAGES.iter().enumerate() {
        let off = 8 + i * 16;
        if let Some((hash, bytes)) = unsafe { lookup(desc.add(off)) } {
            let c = copy.get_or_insert_with(|| {
                let mut b = Box::new([0u8; GRAPHICS_DESC_SIZE]);
                // SAFETY: описание игры имеет размер GRAPHICS_DESC_SIZE.
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
/// `desc` — D3D12_COMPUTE_PIPELINE_STATE_DESC (56 байт).
pub unsafe fn compute_copy(desc: *const u8) -> Option<Box<[u8; COMPUTE_DESC_SIZE]>> {
    let (hash, bytes) = unsafe { lookup(desc.add(8)) }?;
    let mut b = Box::new([0u8; COMPUTE_DESC_SIZE]);
    // SAFETY: описание игры имеет размер COMPUTE_DESC_SIZE.
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

/// Копия потока подобъектов с подменёнными шейдерами (или None).
///
/// # Safety
/// `stream` — поток подобъектов PSO длиной `size`.
pub unsafe fn stream_copy(stream: *const u8, size: usize) -> Option<Vec<u8>> {
    if table().is_empty() {
        return None;
    }
    // SAFETY: поток длиной size от игры.
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

/// Есть ли подмена в потоке подобъектов.
///
/// # Safety
/// `stream` — поток подобъектов PSO длиной `size`.
pub unsafe fn stream_has_override(stream: *const u8, size: usize) -> bool {
    if table().is_empty() {
        return false;
    }
    let mut off = 0usize;
    while off + 4 <= size {
        // SAFETY: внутри потока длиной size.
        let ty = unsafe { std::ptr::read_unaligned(stream.add(off) as *const u32) };
        let Some((data_off, data_size)) = subobject_layout(ty) else { return false };
        if stream_stage(ty).is_some() && unsafe { lookup(stream.add(off + data_off)) }.is_some() {
            return true;
        }
        off += (data_off + data_size).next_multiple_of(8);
    }
    false
}
