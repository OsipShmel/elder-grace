//! «Рецепты» пайплайнов: из чего каждый PSO игры был создан — для перезагрузки подмен с диска.
//!
//! Подмену можно добавить или поправить, пока игра идёт: `grace-ctl overrides reload`
//! перечитывает каталог подмен и пересоздаёт подменённые PSO по рецептам. Рецепт —
//! глубокая копия описания (input layout, stream output, view instancing со своими
//! строками и массивами) **без байткода**: вместо него хеши стадий. Байткод при
//! перезагрузке берётся из каталога подмен или из дампа шейдеров (`GRACE_SHADER_DUMP`),
//! так что корпус (~115 МБ у Elden Ring) в памяти игры не держим: рецепт ≈ 1 КБ.
//!
//! Каждому записанному PSO и его корневой сигнатуре добавляется ссылка: адрес PSO не
//! достанется другому объекту, и пару «оригинал → подменённый» можно ставить в любой
//! момент. Пишем рецепты, только когда задан каталог подмен (режим разработки).

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use crate::{debug, fnv1a, subobject_layout};

#[derive(Clone, Copy)]
pub enum Kind {
    Graphics,
    Compute,
    Stream,
}

pub struct Recipe {
    pub kind: Kind,
    /// Копия описания (graphics/compute) или потока подобъектов; поля байткода обнулены.
    pub bytes: Vec<u8>,
    /// (смещение D3D12_SHADER_BYTECODE в `bytes`, стадия, fnv64 исходного байткода).
    pub shaders: Vec<(usize, &'static str, u64)>,
    /// Строки и массивы, на которые указывает `bytes`.
    _owned: Vec<Box<[u8]>>,
}

static RECIPES: Mutex<Option<HashMap<usize, Recipe>>> = Mutex::new(None);

/// Пишем ли рецепты: только при заданном каталоге подмен.
pub fn active() -> bool {
    static ACTIVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ACTIVE.get_or_init(|| std::env::var_os("GRACE_SHADER_OVERRIDE").is_some())
}

/// Шейдеры PSO по рецепту: (стадия, хеш).
pub fn shaders_of(pso: usize) -> Option<Vec<(&'static str, u64)>> {
    let map = RECIPES.lock().ok()?;
    let r = map.as_ref()?.get(&pso)?;
    Some(r.shaders.iter().map(|&(_, stage, hash)| (stage, hash)).collect())
}

pub fn count() -> usize {
    RECIPES.lock().ok().and_then(|r| r.as_ref().map(HashMap::len)).unwrap_or(0)
}

fn read_usize(b: &[u8], off: usize) -> usize {
    usize::from_le_bytes(b[off..off + 8].try_into().unwrap_or([0; 8]))
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap_or([0; 4]))
}

fn write_usize(b: &mut [u8], off: usize, v: usize) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// Копия `len` байт по `ptr` во владение рецепта; возвращает новый адрес (0 для null).
///
/// # Safety
/// `ptr` читаем на `len` байт или null.
unsafe fn own(owned: &mut Vec<Box<[u8]>>, ptr: usize, len: usize) -> usize {
    if ptr == 0 || len == 0 {
        return 0;
    }
    // SAFETY: по контракту функции.
    let copy: Box<[u8]> = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) }.into();
    let addr = copy.as_ptr() as usize;
    owned.push(copy);
    addr
}

/// Копия C-строки (с нулём).
///
/// # Safety
/// `ptr` — нуль-терминированная строка или null.
unsafe fn own_cstr(owned: &mut Vec<Box<[u8]>>, ptr: usize) -> usize {
    if ptr == 0 {
        return 0;
    }
    // SAFETY: строка нуль-терминирована по контракту.
    let len = unsafe { std::ffi::CStr::from_ptr(ptr as *const std::ffi::c_char) }.to_bytes_with_nul().len();
    unsafe { own(owned, ptr, len) }
}

/// D3D12_SHADER_BYTECODE по смещению `off`: запомнить хеш, обнулить поле.
///
/// # Safety
/// Указатель в `bytes[off..]` — байткод игры, живой на время хука.
unsafe fn take_shader(bytes: &mut [u8], off: usize, stage: &'static str, shaders: &mut Vec<(usize, &'static str, u64)>) {
    let (ptr, len) = (read_usize(bytes, off), read_usize(bytes, off + 8));
    if ptr != 0 && len != 0 && len <= 64 << 20 {
        // SAFETY: по контракту функции.
        let hash = fnv1a(unsafe { std::slice::from_raw_parts(ptr as *const u8, len) });
        shaders.push((off, stage, hash));
    }
    bytes[off..off + 16].fill(0);
}

/// D3D12_INPUT_LAYOUT_DESC { const D3D12_INPUT_ELEMENT_DESC*, UINT } по смещению `off`.
/// Элемент — 32 байта, SemanticName по смещению 0.
///
/// # Safety
/// Указатели в `bytes[off..]` — живые данные игры.
unsafe fn own_input_layout(bytes: &mut [u8], off: usize, owned: &mut Vec<Box<[u8]>>) {
    let (ptr, n) = (read_usize(bytes, off), read_u32(bytes, off + 8) as usize);
    let new = unsafe { own(owned, ptr, n * 32) };
    write_usize(bytes, off, new);
    for i in 0..n {
        let el = (new + i * 32) as *mut usize;
        // SAFETY: элемент лежит в нашей копии (выравнивание не гарантировано); имя — строка игры.
        unsafe { el.write_unaligned(own_cstr(owned, el.read_unaligned())) };
    }
}

/// D3D12_STREAM_OUTPUT_DESC { decl*, UINT n, UINT* strides, UINT nstrides, UINT rasterized }.
/// Элемент объявления — 24 байта, SemanticName по смещению 8.
///
/// # Safety
/// Указатели в `bytes[off..]` — живые данные игры.
unsafe fn own_stream_output(bytes: &mut [u8], off: usize, owned: &mut Vec<Box<[u8]>>) {
    let (decl, n) = (read_usize(bytes, off), read_u32(bytes, off + 8) as usize);
    let new = unsafe { own(owned, decl, n * 24) };
    write_usize(bytes, off, new);
    for i in 0..n {
        let name = (new + i * 24 + 8) as *mut usize;
        // SAFETY: элемент в нашей копии (выравнивание не гарантировано); имя — строка игры.
        unsafe { name.write_unaligned(own_cstr(owned, name.read_unaligned())) };
    }
    let (strides, ns) = (read_usize(bytes, off + 16), read_u32(bytes, off + 24) as usize);
    let new = unsafe { own(owned, strides, ns * 4) };
    write_usize(bytes, off + 16, new);
}

/// AddRef COM-объекта по адресу (слот 1 vtable).
///
/// # Safety
/// `obj` — живой COM-объект или 0.
unsafe fn add_ref(obj: usize) {
    if obj == 0 {
        return;
    }
    // SAFETY: слот 1 vtable любого COM-объекта — AddRef.
    unsafe {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*(*(obj as *const *const usize)).add(1));
        f(obj as *mut c_void);
    }
}

fn insert(pso: *mut c_void, recipe: Recipe) {
    let Ok(mut map) = RECIPES.lock() else { return };
    let map = map.get_or_insert_with(HashMap::new);
    let key = pso as usize;
    if !map.contains_key(&key) {
        // SAFETY: pso только что отдан игре и жив.
        unsafe { add_ref(key) };
    }
    map.insert(key, recipe);
}

/// Смещения в D3D12_GRAPHICS_PIPELINE_STATE_DESC (x64, 656 байт).
const G_STREAM_OUTPUT: usize = 88;
const G_INPUT_LAYOUT: usize = 552;
const G_CACHED_PSO: usize = 632;

/// # Safety
/// `pso` — только что созданный PSO, `desc` — его D3D12_GRAPHICS_PIPELINE_STATE_DESC.
pub unsafe fn record_graphics(pso: *mut c_void, desc: *const u8) {
    if !active() || pso.is_null() || desc.is_null() {
        return;
    }
    // SAFETY: описание игры — 656 байт.
    let mut bytes = unsafe { std::slice::from_raw_parts(desc, crate::overrides::GRAPHICS_DESC_SIZE) }.to_vec();
    let (mut owned, mut shaders) = (Vec::new(), Vec::new());
    unsafe {
        add_ref(read_usize(&bytes, 0));
        for (i, stage) in ["vs", "ps", "ds", "hs", "gs"].into_iter().enumerate() {
            take_shader(&mut bytes, 8 + i * 16, stage, &mut shaders);
        }
        own_stream_output(&mut bytes, G_STREAM_OUTPUT, &mut owned);
        own_input_layout(&mut bytes, G_INPUT_LAYOUT, &mut owned);
    }
    bytes[G_CACHED_PSO..G_CACHED_PSO + 16].fill(0);
    insert(pso, Recipe { kind: Kind::Graphics, bytes, shaders, _owned: owned });
}

/// # Safety
/// `pso` — только что созданный PSO, `desc` — его D3D12_COMPUTE_PIPELINE_STATE_DESC.
pub unsafe fn record_compute(pso: *mut c_void, desc: *const u8) {
    if !active() || pso.is_null() || desc.is_null() {
        return;
    }
    // SAFETY: описание игры — 56 байт.
    let mut bytes = unsafe { std::slice::from_raw_parts(desc, crate::overrides::COMPUTE_DESC_SIZE) }.to_vec();
    let mut shaders = Vec::new();
    unsafe {
        add_ref(read_usize(&bytes, 0));
        take_shader(&mut bytes, 8, "cs", &mut shaders);
    }
    bytes[32..48].fill(0); // CachedPSO
    insert(pso, Recipe { kind: Kind::Compute, bytes, shaders, _owned: Vec::new() });
}

/// # Safety
/// `pso` — только что созданный PSO, `stream` — его поток подобъектов длиной `size`.
pub unsafe fn record_stream(pso: *mut c_void, stream: *const u8, size: usize) {
    if !active() || pso.is_null() || stream.is_null() {
        return;
    }
    // SAFETY: поток игры длиной size.
    let mut bytes = unsafe { std::slice::from_raw_parts(stream, size) }.to_vec();
    let (mut owned, mut shaders) = (Vec::new(), Vec::new());
    let mut off = 0usize;
    while off + 4 <= size {
        let ty = read_u32(&bytes, off);
        let Some((data_off, data_size)) = subobject_layout(ty) else {
            debug(&format!("рецепт: неизвестный подобъект {ty}, PSO не записан"));
            return;
        };
        let d = off + data_off;
        // SAFETY: указатели в подобъектах — живые данные игры на время хука.
        unsafe {
            match ty {
                0 => add_ref(read_usize(&bytes, d)),
                1 => take_shader(&mut bytes, d, "vs", &mut shaders),
                2 => take_shader(&mut bytes, d, "ps", &mut shaders),
                3 => take_shader(&mut bytes, d, "ds", &mut shaders),
                4 => take_shader(&mut bytes, d, "hs", &mut shaders),
                5 => take_shader(&mut bytes, d, "gs", &mut shaders),
                6 => take_shader(&mut bytes, d, "cs", &mut shaders),
                24 => take_shader(&mut bytes, d, "as", &mut shaders),
                25 => take_shader(&mut bytes, d, "ms", &mut shaders),
                7 => own_stream_output(&mut bytes, d, &mut owned),
                12 => own_input_layout(&mut bytes, d, &mut owned),
                19 => bytes[d..d + 16].fill(0),
                22 => {
                    // D3D12_VIEW_INSTANCING_DESC { UINT n; const LOCATION* (8 байт каждая); flags }
                    let n = read_u32(&bytes, d) as usize;
                    let new = own(&mut owned, read_usize(&bytes, d + 8), n * 8);
                    write_usize(&mut bytes, d + 8, new);
                }
                _ => {}
            }
        }
        off += (data_off + data_size).next_multiple_of(8);
    }
    insert(pso, Recipe { kind: Kind::Stream, bytes, shaders, _owned: owned });
}

/// Итог перезагрузки.
#[derive(Default)]
pub struct Rebuilt {
    pub created: usize,
    pub failed: usize,
    pub cleared: usize,
}

/// Пересоздаёт подменённые PSO по рецептам с текущей таблицей подмен.
///
/// `create(kind, bytes, out)` создаёт PSO из готового описания/потока.
/// `original(stage, hash)` — исходный байткод стадии (из дампа).
pub fn rebuild(
    lookup: impl Fn(u64) -> Option<&'static [u8]>,
    original: impl Fn(&str, u64) -> Option<Vec<u8>>,
    create: impl Fn(Kind, &[u8], *mut *mut c_void) -> i32,
    mut set_pair: impl FnMut(usize, Option<usize>) -> bool,
) -> Rebuilt {
    let mut res = Rebuilt::default();
    let Ok(map) = RECIPES.lock() else { return res };
    let Some(map) = map.as_ref() else { return res };
    for (&key, r) in map {
        if !r.shaders.iter().any(|&(_, _, h)| lookup(h).is_some()) {
            if set_pair(key, None) {
                res.cleared += 1;
            }
            continue;
        }
        let mut bytes = r.bytes.clone();
        let mut keep: Vec<Vec<u8>> = Vec::new();
        let mut ok = true;
        for &(off, stage, hash) in &r.shaders {
            let (ptr, len) = match lookup(hash) {
                Some(b) => (b.as_ptr() as usize, b.len()),
                None => match original(stage, hash) {
                    Some(v) => {
                        let p = (v.as_ptr() as usize, v.len());
                        keep.push(v);
                        p
                    }
                    None => {
                        debug(&format!("перезагрузка: нет исходного {hash:016x}.{stage} в дампе"));
                        ok = false;
                        break;
                    }
                },
            };
            write_usize(&mut bytes, off, ptr);
            write_usize(&mut bytes, off + 8, len);
        }
        let mut out = std::ptr::null_mut();
        let hr = if ok { create(r.kind, &bytes, &mut out) } else { -1 };
        drop(keep);
        if hr >= 0 && !out.is_null() {
            set_pair(key, Some(out as usize));
            res.created += 1;
        } else {
            if ok {
                debug(&format!("перезагрузка: PSO не создан ({hr:#x})"));
            }
            set_pair(key, None);
            res.failed += 1;
        }
    }
    res
}
