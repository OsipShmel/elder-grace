//! Shim Elder Grace внутри игры: прокси `d3d12.dll`.
//!
//! Кладётся в песочницу игры как `d3d12.dll` (override `d3d12=native,builtin`),
//! исполняется под Rosetta внутри процесса игры, поэтому держим его тонким.
//!
//! - Все экспорты настоящего `d3d12.dll` (D3DMetal из GPTK) — naked-трамплины,
//!   ординалы как у Windows (см. build.rs): игры импортируют `D3D12CreateDevice` по №101.
//! - `D3D12CreateDevice` после успеха патчит vtable устройства: `CreateGraphicsPipelineState`,
//!   `CreateComputePipelineState`, `CreatePipelineState` (поток подобъектов, ID3D12Device2).
//! - Каждый шейдер пайплайна пишется в каталог `GRACE_SHADER_DUMP` как
//!   `<fnv64>.<стадия>.dxbc` (без повторов); сводка — в `_stats.txt` там же.
//! - Подмены шейдеров (`overrides.rs`) включаются и выключаются на лету (`live.rs`):
//!   командным спискам подставляется своя vtable, управление по TCP 47322.

#![cfg(all(windows, target_arch = "x86_64"))]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::io::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

mod live;
mod overrides;
mod recipes;
mod resources;

/// Версия протокола shim ↔ рантайм (совпадает с `grace_proto::PROTOCOL_VERSION`).
pub const SHIM_PROTOCOL_VERSION: u32 = 1;

#[unsafe(no_mangle)]
pub extern "C" fn grace_shim_version() -> u32 {
    SHIM_PROTOCOL_VERSION
}

// ---- проброс экспортов ------------------------------------------------------------------

const NAMES: [&str; 11] = [
    "GetBehaviorValue",
    "D3D12CreateDevice",
    "D3D12GetDebugInterface",
    "D3D12CoreCreateLayeredDevice",
    "D3D12CoreGetLayeredDeviceSize",
    "D3D12CoreRegisterLayers",
    "D3D12CreateRootSignatureDeserializer",
    "D3D12CreateVersionedRootSignatureDeserializer",
    "D3D12EnableExperimentalFeatures",
    "D3D12SerializeRootSignature",
    "D3D12SerializeVersionedRootSignature",
];
const IDX_CREATE_DEVICE: usize = 1;

static REAL: [AtomicUsize; NAMES.len()] = [const { AtomicUsize::new(0) }; NAMES.len()];

macro_rules! thunk {
    ($name:ident, $idx:expr) => {
        #[unsafe(no_mangle)]
        #[unsafe(naked)]
        pub unsafe extern "system" fn $name() {
            core::arch::naked_asm!(
                "jmp qword ptr [rip + {real} + {off}]",
                real = sym crate::REAL,
                off = const $idx * 8,
            );
        }
    };
}

thunk!(GetBehaviorValue, 0);
thunk!(D3D12GetDebugInterface, 2);
thunk!(D3D12CoreCreateLayeredDevice, 3);
thunk!(D3D12CoreGetLayeredDeviceSize, 4);
thunk!(D3D12CoreRegisterLayers, 5);
thunk!(D3D12CreateRootSignatureDeserializer, 6);
thunk!(D3D12CreateVersionedRootSignatureDeserializer, 7);
thunk!(D3D12EnableExperimentalFeatures, 8);
thunk!(D3D12SerializeRootSignature, 9);
thunk!(D3D12SerializeVersionedRootSignature, 10);

unsafe extern "system" {
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn OutputDebugStringA(s: *const u8);
}

pub(crate) fn debug(msg: &str) {
    let mut s = Vec::with_capacity(msg.len() + 32);
    s.extend_from_slice(b"[grace d3d12] ");
    s.extend_from_slice(msg.as_bytes());
    s.extend_from_slice(b"\n\0");
    // SAFETY: строка нуль-терминирована.
    unsafe { OutputDebugStringA(s.as_ptr()) };
}

fn resolve_real() {
    let mut dir = [0u16; 260];
    // SAFETY: буфер на 260 u16.
    let len = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
    let mut path: Vec<u16> = dir[..len.min(dir.len())].to_vec();
    path.extend("\\d3d12.dll\0".encode_utf16());
    // SAFETY: путь нуль-терминирован; это builtin d3d12 (D3DMetal), полный путь отличается от нашего.
    let real = unsafe { LoadLibraryW(path.as_ptr()) };
    let mut found = 0;
    for (slot, name) in REAL.iter().zip(NAMES) {
        let cname: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
        // SAFETY: модуль загружен (или null — тогда GetProcAddress вернёт null).
        let addr = if real.is_null() { std::ptr::null_mut() } else { unsafe { GetProcAddress(real, cname.as_ptr()) } };
        if !addr.is_null() {
            found += 1;
        }
        slot.store(addr as usize, Ordering::Release);
    }
    debug(&format!("загружен поверх системного d3d12.dll, функций {found}/{}", NAMES.len()));
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_m: *mut c_void, reason: u32, _r: *mut c_void) -> i32 {
    if reason == 1 {
        resolve_real();
    }
    1
}

// ---- D3D12CreateDevice и vtable устройства ------------------------------------------------

type CreateDevice = unsafe extern "system" fn(*mut c_void, u32, *const [u8; 16], *mut *mut c_void) -> i32;
type CreatePso = unsafe extern "system" fn(*mut c_void, *const u8, *const [u8; 16], *mut *mut c_void) -> i32;
type QueryInterface = unsafe extern "system" fn(*mut c_void, *const [u8; 16], *mut *mut c_void) -> i32;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;

// Слоты vtable ID3D12Device*: IUnknown(0–2), ID3D12Object(3–6), ID3D12Device(7–43),
// ID3D12Device1(44–46), ID3D12Device2::CreatePipelineState — 47.
const SLOT_GRAPHICS: usize = 10;
const SLOT_COMPUTE: usize = 11;
const SLOT_STREAM: usize = 47;

/// IID_ID3D12Device2 {30baa41e-b15b-475c-a0bb-1af5c5b64328}.
const IID_DEVICE2: [u8; 16] = [0x1e, 0xa4, 0xba, 0x30, 0x5b, 0xb1, 0x5c, 0x47, 0xa0, 0xbb, 0x1a, 0xf5, 0xc5, 0xb6, 0x43, 0x28];

static ORIG_GRAPHICS: AtomicUsize = AtomicUsize::new(0);
static ORIG_COMPUTE: AtomicUsize = AtomicUsize::new(0);
static ORIG_STREAM: AtomicUsize = AtomicUsize::new(0);
static ORIG_QI: AtomicUsize = AtomicUsize::new(0);
static ORIG_LIBRARY: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_LIST: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_LIST1: AtomicUsize = AtomicUsize::new(0);
const SLOT_QI: usize = 0;
const SLOT_LIBRARY: usize = 44;
/// ID3D12Device::CreateCommandList (d3d12.h).
const SLOT_CREATE_LIST: usize = 12;
/// ID3D12Device4::CreateCommandList1 (d3d12.h).
const SLOT_CREATE_LIST1: usize = 51;
/// IID_ID3D12Device4 {e865df17-a9ee-46f9-a463-3098315aa2e5}.
const IID_DEVICE4: [u8; 16] = live::guid(0xe865df17, 0xa9ee, 0x46f9, [0xa4, 0x63, 0x30, 0x98, 0x31, 0x5a, 0xa2, 0xe5]);

unsafe extern "system" fn hooked_create_list(
    this: *mut c_void,
    node_mask: u32,
    ty: u32,
    allocator: *mut c_void,
    initial: *mut c_void,
    riid: *const [u8; 16],
    out: *mut *mut c_void,
) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, u32, u32, *mut c_void, *mut c_void, *const [u8; 16], *mut *mut c_void) -> i32;
    // SAFETY: оригинал записан при установке хука.
    let orig: F = unsafe { std::mem::transmute(ORIG_CREATE_LIST.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, node_mask, ty, allocator, live::initial_pso(initial), riid, out) };
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — командный список.
        unsafe { live::adopt_command_list(*out) };
    }
    hr
}

unsafe extern "system" fn hooked_create_list1(this: *mut c_void, node_mask: u32, ty: u32, flags: u32, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, u32, u32, u32, *const [u8; 16], *mut *mut c_void) -> i32;
    // SAFETY: оригинал записан при установке хука.
    let orig: F = unsafe { std::mem::transmute(ORIG_CREATE_LIST1.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, node_mask, ty, flags, riid, out) };
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — командный список.
        unsafe { live::adopt_command_list(*out) };
    }
    hr
}

/// Создаёт подменённый PSO вторым и регистрирует пару с оригиналом, который уже у игры.
///
/// # Safety
/// `out` — выходной указатель успешного вызова создания PSO, `create` создаёт подменённый.
unsafe fn make_pair(out: *mut *mut c_void, create: impl FnOnce(*mut *mut c_void) -> i32) {
    if out.is_null() {
        return;
    }
    // SAFETY: вызывающий передал out успешного вызова.
    let orig = unsafe { *out };
    let mut over = std::ptr::null_mut();
    let hr = create(&mut over);
    if hr >= 0 && !over.is_null() {
        live::set_pair(orig as usize, Some(over as usize));
    } else {
        live::note_failed();
        debug(&format!("подменённый PSO не создан ({hr:#x}), игра работает с оригиналом"));
    }
}

/// Диагностика: какие интерфейсы игра просит у устройства и тот же ли объект ей отдают.
unsafe extern "system" fn hooked_qi(this: *mut c_void, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    // SAFETY: оригинал записан при установке хука.
    let orig: QueryInterface = unsafe { std::mem::transmute(ORIG_QI.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, riid, out) };
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — COM-объект.
        unsafe { adopt(*out) };
    }
    static N: AtomicU64 = AtomicU64::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 40 && !riid.is_null() && !out.is_null() {
        // SAFETY: riid — 16 байт GUID, *out валиден при успехе.
        let g = unsafe { *riid };
        let same = hr >= 0 && unsafe { *out } == this;
        let vt_same = hr >= 0 && unsafe { !(*out).is_null() && *(*out as *const usize) == *(this as *const usize) };
        debug(&format!(
            "QueryInterface {{{:02x}{:02x}{:02x}{:02x}-…}} → {hr:#x}, тот же объект: {same}, та же vtable: {vt_same}",
            g[3], g[2], g[1], g[0]
        ));
    }
    hr
}

unsafe extern "system" fn hooked_library(this: *mut c_void, blob: *const u8, len: usize, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *const u8, usize, *const [u8; 16], *mut *mut c_void) -> i32;
    // SAFETY: оригинал записан при установке хука.
    let orig: F = unsafe { std::mem::transmute(ORIG_LIBRARY.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, blob, len, riid, out) };
    debug(&format!("CreatePipelineLibrary(кэш {len} байт) → {hr:#x}"));
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — объект ID3D12PipelineLibrary.
        unsafe { hook_library(*out) };
    }
    hr
}

// ---- ID3D12PipelineLibrary: игра грузит большую часть PSO из своего кэша ---------------------
// Слоты: IUnknown(0–2), ID3D12Object(3–6), GetDevice(7), StorePipeline(8),
// LoadGraphicsPipeline(9), LoadComputePipeline(10), GetSerializedSize(11), Serialize(12),
// ID3D12PipelineLibrary1::LoadPipeline(13).
const LIB_SLOTS: usize = 16;
const LIB_LOAD_GRAPHICS: usize = 9;
const LIB_LOAD_COMPUTE: usize = 10;
const LIB_LOAD_STREAM: usize = 13;
/// IID_ID3D12PipelineLibrary1 {80eabf42-2568-4e5e-bd82-c37f86961dc3}.
const IID_LIBRARY1: [u8; 16] = [0x42, 0xbf, 0xea, 0x80, 0x68, 0x25, 0x5e, 0x4e, 0xbd, 0x82, 0xc3, 0x7f, 0x86, 0x96, 0x1d, 0xc3];

static LIB_ORIG_VTBL: AtomicUsize = AtomicUsize::new(0);
static LIB_CLONED_VTBL: AtomicUsize = AtomicUsize::new(0);
static ORIG_LIB_GRAPHICS: AtomicUsize = AtomicUsize::new(0);
static ORIG_LIB_COMPUTE: AtomicUsize = AtomicUsize::new(0);
static ORIG_LIB_STREAM: AtomicUsize = AtomicUsize::new(0);

type LibLoad = unsafe extern "system" fn(*mut c_void, *const u16, *const u8, *const [u8; 16], *mut *mut c_void) -> i32;

unsafe fn hook_library(lib: *mut c_void) {
    if lib.is_null() {
        return;
    }
    // SAFETY: первое поле COM-объекта — указатель на vtable.
    let vtbl = unsafe { *(lib as *const *const usize) };
    // Все библиотеки делят одну vtable: копию делаем один раз, дальше только подставляем.
    if LIB_ORIG_VTBL.load(Ordering::Acquire) == 0 {
        let qi: QueryInterface = unsafe { std::mem::transmute(*vtbl) };
        let release: Release = unsafe { std::mem::transmute(*vtbl.add(2)) };
        let mut l1 = std::ptr::null_mut();
        let hr = unsafe { qi(lib, &IID_LIBRARY1, &mut l1) };
        let has_stream = hr >= 0 && l1 == lib;
        if !l1.is_null() {
            unsafe { release(l1) };
        }
        let mut clone: Box<[usize; LIB_SLOTS]> = Box::new([0; LIB_SLOTS]);
        // SAFETY: читаем LIB_SLOTS указателей vtable (константные данные фреймворка).
        unsafe { std::ptr::copy_nonoverlapping(vtbl, clone.as_mut_ptr(), LIB_SLOTS) };
        ORIG_LIB_GRAPHICS.store(clone[LIB_LOAD_GRAPHICS], Ordering::Release);
        ORIG_LIB_COMPUTE.store(clone[LIB_LOAD_COMPUTE], Ordering::Release);
        clone[LIB_LOAD_GRAPHICS] = hooked_lib_graphics as *const () as usize;
        clone[LIB_LOAD_COMPUTE] = hooked_lib_compute as *const () as usize;
        if has_stream {
            ORIG_LIB_STREAM.store(clone[LIB_LOAD_STREAM], Ordering::Release);
            clone[LIB_LOAD_STREAM] = hooked_lib_stream as *const () as usize;
        }
        let clone = Box::leak(clone);
        LIB_CLONED_VTBL.store(clone.as_ptr() as usize, Ordering::Release);
        LIB_ORIG_VTBL.store(vtbl as usize, Ordering::Release);
        debug(&format!("библиотеке пайплайнов подставлена своя vtable (LoadPipeline: {})", if has_stream { "да" } else { "нет" }));
    }
    if vtbl as usize == LIB_ORIG_VTBL.load(Ordering::Acquire) {
        // SAFETY: объект в куче, писать в его первое поле можно.
        unsafe { std::ptr::write_volatile(lib as *mut usize, LIB_CLONED_VTBL.load(Ordering::Acquire)) };
    }
}

unsafe extern "system" fn hooked_lib_graphics(this: *mut c_void, name: *const u16, desc: *const u8, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    if !desc.is_null() {
        for (i, stage) in ["vs", "ps", "ds", "hs", "gs"].iter().enumerate() {
            // SAFETY: тот же D3D12_GRAPHICS_PIPELINE_STATE_DESC, что у CreateGraphicsPipelineState.
            unsafe { dump_bytecode(stage, desc.add(8 + i * 16)) };
        }
        note_pipeline(Kind::Library);
        if unsafe { overrides::graphics_has_override(desc) } {
            return library_miss(out);
        }
    }
    // SAFETY: оригинал записан при подстановке vtable.
    let orig: LibLoad = unsafe { std::mem::transmute(ORIG_LIB_GRAPHICS.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, name, desc, riid, out) };
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — PSO из кэша, desc — его описание.
        unsafe { recipes::record_graphics(*out, desc) };
    }
    hr
}

unsafe extern "system" fn hooked_lib_compute(this: *mut c_void, name: *const u16, desc: *const u8, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    if !desc.is_null() {
        unsafe { dump_bytecode("cs", desc.add(8)) };
        note_pipeline(Kind::Library);
        if unsafe { overrides::compute_has_override(desc) } {
            return library_miss(out);
        }
    }
    // SAFETY: оригинал записан при подстановке vtable.
    let orig: LibLoad = unsafe { std::mem::transmute(ORIG_LIB_COMPUTE.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, name, desc, riid, out) };
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — PSO из кэша, desc — его описание.
        unsafe { recipes::record_compute(*out, desc) };
    }
    hr
}

unsafe extern "system" fn hooked_lib_stream(this: *mut c_void, name: *const u16, desc: *const u8, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    if !desc.is_null() {
        // SAFETY: D3D12_PIPELINE_STATE_STREAM_DESC { SIZE_T, void* }.
        let (size, stream) = unsafe { (*(desc as *const usize), *(desc.add(8) as *const *const u8)) };
        if !stream.is_null() {
            unsafe { parse_stream(stream, size) };
            if unsafe { overrides::stream_has_override(stream, size) } {
                note_pipeline(Kind::Library);
                return library_miss(out);
            }
        }
        note_pipeline(Kind::Library);
    }
    // SAFETY: оригинал записан при подстановке vtable.
    let orig: LibLoad = unsafe { std::mem::transmute(ORIG_LIB_STREAM.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, name, desc, riid, out) };
    if hr >= 0 && !out.is_null() && !desc.is_null() {
        // SAFETY: D3D12_PIPELINE_STATE_STREAM_DESC { SIZE_T, void* }; *out — PSO из кэша.
        let (size, stream) = unsafe { (*(desc as *const usize), *(desc.add(8) as *const *const u8)) };
        unsafe { recipes::record_stream(*out, stream, size) };
    }
    hr
}


/// Промах в кэше пайплайнов ради подмены: игра создаст пайплайн через устройство.
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

fn library_miss(out: *mut *mut c_void) -> i32 {
    static ONCE: AtomicBool = AtomicBool::new(false);
    if !ONCE.swap(true, Ordering::Relaxed) {
        debug("кэш пайплайнов: подменённые PSO отдаём как промах, игра создаст их заново");
    }
    if !out.is_null() {
        // SAFETY: out — выходной указатель вызывающего.
        unsafe { *out = std::ptr::null_mut() };
    }
    E_INVALIDARG
}

static HOOKED: AtomicBool = AtomicBool::new(false);
/// Устройство игры: на нём пересоздаём подменённые PSO при перезагрузке.
static DEVICE: AtomicUsize = AtomicUsize::new(0);
/// IID_ID3D12PipelineState {765a30f3-f624-4c6f-a828-ace948622445}.
const IID_PSO: [u8; 16] = live::guid(0x765a30f3, 0xf624, 0x4c6f, [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45]);

/// `grace-ctl overrides reload`: перечитать подмены и пересоздать подменённые PSO.
pub(crate) fn reload_overrides() -> String {
    let dev = DEVICE.load(Ordering::Acquire) as *mut c_void;
    if dev.is_null() {
        return "перезагрузка: устройства ещё нет".into();
    }
    if !recipes::active() {
        return "перезагрузка: каталог подмен не задан (GRACE_SHADER_OVERRIDE)".into();
    }
    let started = std::time::Instant::now();
    let loaded = overrides::reload();
    let original = |stage: &str, hash: u64| std::fs::read(dump_dir()?.join(format!("{hash:016x}.{stage}.dxbc"))).ok();
    let create = |kind: recipes::Kind, bytes: &[u8], out: *mut *mut c_void| -> i32 {
        // SAFETY: оригиналы методов устройства записаны в hook_device; описание собрано из рецепта.
        unsafe {
            match kind {
                recipes::Kind::Graphics => {
                    let f: CreatePso = std::mem::transmute(ORIG_GRAPHICS.load(Ordering::Acquire));
                    f(dev, bytes.as_ptr(), &IID_PSO, out)
                }
                recipes::Kind::Compute => {
                    let f: CreatePso = std::mem::transmute(ORIG_COMPUTE.load(Ordering::Acquire));
                    f(dev, bytes.as_ptr(), &IID_PSO, out)
                }
                recipes::Kind::Stream => {
                    let p = ORIG_STREAM.load(Ordering::Acquire);
                    if p == 0 {
                        return E_INVALIDARG;
                    }
                    let f: CreatePso = std::mem::transmute(p);
                    let desc: [usize; 2] = [bytes.len(), bytes.as_ptr() as usize];
                    f(dev, desc.as_ptr().cast(), &IID_PSO, out)
                }
            }
        }
    };
    let r = recipes::rebuild(overrides::get, original, create, live::set_pair);
    format!(
        "перезагрузка: подмен {loaded}, PSO пересоздано {}, ошибок {}, снято {} за {} мс",
        r.created,
        r.failed,
        r.cleared,
        started.elapsed().as_millis()
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12CreateDevice(adapter: *mut c_void, level: u32, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    let real = REAL[IDX_CREATE_DEVICE].load(Ordering::Acquire);
    if real == 0 {
        return 0x8000_4005_u32 as i32; // E_FAIL
    }
    // SAFETY: адрес настоящей функции из системного d3d12.dll.
    let real: CreateDevice = unsafe { std::mem::transmute(real) };
    let hr = unsafe { real(adapter, level, riid, out) };
    debug(&format!("D3D12CreateDevice(уровень {level:#x}) → {hr:#x}"));
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — указатель на COM-объект устройства.
        let dev = unsafe { *out };
        if !dev.is_null() {
            unsafe { hook_device(dev) };
        }
    }
    hr
}

/// Сколько слотов vtable копируем: с запасом на ID3D12Device до 14-й версии (~80 методов).
const VTBL_SLOTS: usize = 128;

/// Наша копия vtable устройства. vtable D3DMetal лежит в read-only памяти маковского
/// фреймворка, `VirtualProtect` Wine её не меняет — поэтому объекту подставляется копия.
static CLONED_VTBL: AtomicUsize = AtomicUsize::new(0);
static ORIG_VTBL: AtomicUsize = AtomicUsize::new(0);

/// Подменяет указатель vtable у объекта на нашу копию (если это объект с исходной vtable).
unsafe fn adopt(obj: *mut c_void) {
    let (orig, clone) = (ORIG_VTBL.load(Ordering::Acquire), CLONED_VTBL.load(Ordering::Acquire));
    if obj.is_null() || orig == 0 || clone == 0 {
        return;
    }
    // SAFETY: первое поле COM-объекта — указатель на vtable; сам объект лежит в куче, писать можно.
    unsafe {
        let p = obj as *mut usize;
        if *p == orig {
            std::ptr::write_volatile(p, clone);
        }
    }
}

unsafe fn hook_device(dev: *mut c_void) {
    if HOOKED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: первое поле COM-объекта — указатель на vtable.
    let vtbl = unsafe { *(dev as *const *const usize) };
    // Слот 47 есть только у ID3D12Device2+: проверяем, что объект отдаёт этот интерфейс
    // тем же указателем (значит, vtable общая и длиннее 47 слотов).
    // SAFETY: слоты 0 и 2 — QueryInterface и Release любого COM-объекта.
    let qi: QueryInterface = unsafe { std::mem::transmute(*vtbl) };
    let release: Release = unsafe { std::mem::transmute(*vtbl.add(2)) };
    let mut d2 = std::ptr::null_mut();
    let hr = unsafe { qi(dev, &IID_DEVICE2, &mut d2) };
    let stream = hr >= 0 && d2 == dev;
    if !d2.is_null() {
        unsafe { release(d2) };
    }
    let mut d4 = std::ptr::null_mut();
    let hr = unsafe { qi(dev, &IID_DEVICE4, &mut d4) };
    let device4 = hr >= 0 && d4 == dev;
    if !d4.is_null() {
        unsafe { release(d4) };
    }

    // SAFETY: читаем VTBL_SLOTS указателей из vtable (константные данные фреймворка).
    let mut clone: Box<[usize; VTBL_SLOTS]> = Box::new([0; VTBL_SLOTS]);
    unsafe { std::ptr::copy_nonoverlapping(vtbl, clone.as_mut_ptr(), VTBL_SLOTS) };
    let mut set = |slot: usize, hook: usize, orig: &AtomicUsize| {
        orig.store(clone[slot], Ordering::Release);
        clone[slot] = hook;
    };
    set(SLOT_QI, hooked_qi as *const () as usize, &ORIG_QI);
    set(SLOT_GRAPHICS, hooked_graphics as *const () as usize, &ORIG_GRAPHICS);
    set(SLOT_COMPUTE, hooked_compute as *const () as usize, &ORIG_COMPUTE);
    set(SLOT_CREATE_LIST, hooked_create_list as *const () as usize, &ORIG_CREATE_LIST);
    if device4 {
        set(SLOT_CREATE_LIST1, hooked_create_list1 as *const () as usize, &ORIG_CREATE_LIST1);
    }
    if stream {
        set(SLOT_LIBRARY, hooked_library as *const () as usize, &ORIG_LIBRARY);
        set(SLOT_STREAM, hooked_stream as *const () as usize, &ORIG_STREAM);
    }
    let clone = Box::leak(clone);
    DEVICE.store(dev as usize, Ordering::Release);
    ORIG_VTBL.store(vtbl as usize, Ordering::Release);
    CLONED_VTBL.store(clone.as_ptr() as usize, Ordering::Release);
    unsafe { adopt(dev) };
    debug(&format!(
        "устройству подставлена своя vtable (потоковые PSO: {}, CreateCommandList1: {})",
        if stream { "да" } else { "нет" },
        if device4 { "да" } else { "нет" }
    ));
    live::start_control();
}

unsafe extern "system" fn hooked_graphics(this: *mut c_void, desc: *const u8, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    if !desc.is_null() {
        // D3D12_GRAPHICS_PIPELINE_STATE_DESC: pRootSignature, затем VS, PS, DS, HS, GS по 16 байт.
        for (i, stage) in ["vs", "ps", "ds", "hs", "gs"].iter().enumerate() {
            // SAFETY: поля по документированному смещению структуры.
            unsafe { dump_bytecode(stage, desc.add(8 + i * 16)) };
        }
        note_pipeline(Kind::Graphics);
    }
    // SAFETY: оригинал записан при установке хука.
    let orig: CreatePso = unsafe { std::mem::transmute(ORIG_GRAPHICS.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, desc, riid, out) };
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — PSO, desc — его описание.
        unsafe { recipes::record_graphics(*out, desc) };
    }
    if hr >= 0
        && !desc.is_null()
        && let Some(copy) = unsafe { overrides::graphics_copy(desc) }
    {
        unsafe { make_pair(out, |o| orig(this, copy.as_ptr(), riid, o)) };
    }
    hr
}

unsafe extern "system" fn hooked_compute(this: *mut c_void, desc: *const u8, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    if !desc.is_null() {
        // D3D12_COMPUTE_PIPELINE_STATE_DESC: pRootSignature, CS.
        unsafe { dump_bytecode("cs", desc.add(8)) };
        note_pipeline(Kind::Compute);
    }
    // SAFETY: оригинал записан при установке хука.
    let orig: CreatePso = unsafe { std::mem::transmute(ORIG_COMPUTE.load(Ordering::Acquire)) };
    let hr = unsafe { orig(this, desc, riid, out) };
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — PSO, desc — его описание.
        unsafe { recipes::record_compute(*out, desc) };
    }
    if hr >= 0
        && !desc.is_null()
        && let Some(copy) = unsafe { overrides::compute_copy(desc) }
    {
        unsafe { make_pair(out, |o| orig(this, copy.as_ptr(), riid, o)) };
    }
    hr
}

unsafe extern "system" fn hooked_stream(this: *mut c_void, desc: *const u8, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    // SAFETY: оригинал записан при установке хука.
    let orig: CreatePso = unsafe { std::mem::transmute(ORIG_STREAM.load(Ordering::Acquire)) };
    if desc.is_null() {
        return unsafe { orig(this, desc, riid, out) };
    }
    // D3D12_PIPELINE_STATE_STREAM_DESC { SIZE_T SizeInBytes; void* pPipelineStateSubobjectStream; }
    // SAFETY: структура из двух полей по 8 байт.
    let (size, stream) = unsafe { (*(desc as *const usize), *(desc.add(8) as *const *const u8)) };
    if !stream.is_null() {
        unsafe { parse_stream(stream, size) };
    }
    note_pipeline(Kind::Stream);
    let hr = unsafe { orig(this, desc, riid, out) };
    if hr >= 0 && !out.is_null() && !stream.is_null() {
        // SAFETY: при успехе *out — PSO, stream — его поток подобъектов.
        unsafe { recipes::record_stream(*out, stream, size) };
    }
    if hr >= 0
        && !stream.is_null()
        && let Some(copy) = unsafe { overrides::stream_copy(stream, size) }
    {
        // Новое D3D12_PIPELINE_STATE_STREAM_DESC на нашу копию потока.
        let new_desc: [usize; 2] = [size, copy.as_ptr() as usize];
        unsafe { make_pair(out, |o| orig(this, new_desc.as_ptr().cast(), riid, o)) };
    }
    hr
}

/// (смещение данных от начала подобъекта, размер данных) по типу подобъекта.
pub(crate) fn subobject_layout(ty: u32) -> Option<(usize, usize)> {
    Some(match ty {
        0 => (8, 8),               // ROOT_SIGNATURE
        1..=6 | 24 | 25 => (8, 16), // VS PS DS HS GS CS AS MS
        7 => (8, 32),              // STREAM_OUTPUT
        8 => (4, 328),             // BLEND
        9 | 13 | 14 | 16 | 18 | 20 => (4, 4),
        10 => (4, 44),             // RASTERIZER
        11 => (4, 52),             // DEPTH_STENCIL
        12 => (8, 16),             // INPUT_LAYOUT
        15 => (4, 36),             // RENDER_TARGET_FORMATS
        17 => (4, 8),              // SAMPLE_DESC
        19 => (8, 16),             // CACHED_PSO
        21 => (4, 56),             // DEPTH_STENCIL1
        22 => (8, 24),             // VIEW_INSTANCING
        _ => return None,
    })
}

unsafe fn parse_stream(stream: *const u8, size: usize) {
    let mut off = 0usize;
    while off + 4 <= size {
        // SAFETY: читаем внутри переданного потока размера size.
        let ty = unsafe { std::ptr::read_unaligned(stream.add(off) as *const u32) };
        let Some((data_off, data_size)) = subobject_layout(ty) else {
            static ONCE: AtomicBool = AtomicBool::new(false);
            if !ONCE.swap(true, Ordering::Relaxed) {
                debug(&format!("неизвестный тип подобъекта PSO {ty}, разбор потока остановлен"));
            }
            return;
        };
        let stage = match ty {
            1 => Some("vs"),
            2 => Some("ps"),
            3 => Some("ds"),
            4 => Some("hs"),
            5 => Some("gs"),
            6 => Some("cs"),
            24 => Some("as"),
            25 => Some("ms"),
            _ => None,
        };
        if let Some(stage) = stage {
            unsafe { dump_bytecode(stage, stream.add(off + data_off)) };
        }
        off += (data_off + data_size).next_multiple_of(8);
    }
}

// ---- дамп и статистика ----------------------------------------------------------------------

enum Kind {
    Graphics,
    Compute,
    Stream,
    /// Загрузка PSO из ID3D12PipelineLibrary (кэш игры).
    Library,
}

static PIPELINES: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static SHADERS_SEEN: AtomicU64 = AtomicU64::new(0);
static SHADERS_NEW: AtomicU64 = AtomicU64::new(0);
static BYTES_NEW: AtomicU64 = AtomicU64::new(0);

pub(crate) fn dump_dir() -> Option<&'static std::path::Path> {
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::path::PathBuf::from(std::env::var_os("GRACE_SHADER_DUMP")?);
        std::fs::create_dir_all(&dir).ok()?;
        debug(&format!("дамп шейдеров в {}", dir.display()));
        Some(dir)
    })
    .as_deref()
}

pub(crate) fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// `bc` указывает на D3D12_SHADER_BYTECODE { const void* pShaderBytecode; SIZE_T BytecodeLength; }.
unsafe fn dump_bytecode(stage: &str, bc: *const u8) {
    // SAFETY: структура из указателя и длины.
    let (ptr, len) = unsafe { (*(bc as *const *const u8), *(bc.add(8) as *const usize)) };
    if ptr.is_null() || len == 0 || len > 64 << 20 {
        return;
    }
    SHADERS_SEEN.fetch_add(1, Ordering::Relaxed);
    let Some(dir) = dump_dir() else { return };
    // SAFETY: байткод шейдера длиной len, переданный игрой в D3D12.
    let data = unsafe { std::slice::from_raw_parts(ptr, len) };
    let path = dir.join(format!("{:016x}.{stage}.dxbc", fnv1a(data)));
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
        && f.write_all(data).is_ok()
    {
        SHADERS_NEW.fetch_add(1, Ordering::Relaxed);
        BYTES_NEW.fetch_add(len as u64, Ordering::Relaxed);
    }
}

fn note_pipeline(kind: Kind) {
    let i = match kind {
        Kind::Graphics => 0,
        Kind::Compute => 1,
        Kind::Stream => 2,
        Kind::Library => 3,
    };
    let n = PIPELINES[i].fetch_add(1, Ordering::Relaxed) + 1;
    let total: u64 = PIPELINES.iter().map(|p| p.load(Ordering::Relaxed)).sum();
    if total == 1 || total.is_multiple_of(25) || n == 1 {
        let stats = format!(
            "пайплайнов: graphics {} · compute {} · stream {} · из библиотеки {}\nшейдеров увидено {} · новых в дампе {} ({} КБ)\n",
            PIPELINES[0].load(Ordering::Relaxed),
            PIPELINES[1].load(Ordering::Relaxed),
            PIPELINES[2].load(Ordering::Relaxed),
            PIPELINES[3].load(Ordering::Relaxed),
            SHADERS_SEEN.load(Ordering::Relaxed),
            SHADERS_NEW.load(Ordering::Relaxed),
            BYTES_NEW.load(Ordering::Relaxed) / 1024,
        );
        debug(&stats.replace('\n', " | "));
        if let Some(dir) = dump_dir() {
            let _ = std::fs::write(dir.join("_stats.txt"), stats);
        }
    }
}
