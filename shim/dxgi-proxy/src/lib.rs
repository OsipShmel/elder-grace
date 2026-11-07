//! Shim Elder Grace внутри игры: прокси `dxgi.dll`.
//!
//! Задача — время кадра. Кадр в D3D12 заканчивается `IDXGISwapChain::Present`/`Present1`,
//! поэтому перехватываем создание фабрики → создание swapchain → Present.
//! vtable объектов D3DMetal лежат в read-only памяти маковского фреймворка, поэтому, как и
//! в `d3d12.dll`-shim, объектам подставляется своя копия vtable.
//!
//! - Журнал: `GRACE_FRAMETIME_LOG` — CSV `frame,t_ms,dt_ms` (t от первого Present).
//! - Живая статистика: TCP `127.0.0.1:GRACE_SHIM_PORT` (по умолчанию 47321), протокол
//!   `grace-proto` (`ping`, `stats`), читается `grace-ctl --tcp`.

#![cfg(all(windows, target_arch = "x86_64"))]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

mod frames;

// ---- проброс экспортов ------------------------------------------------------------------

const NAMES: [&str; 7] = [
    "CreateDXGIFactory",
    "CreateDXGIFactory1",
    "CreateDXGIFactory2",
    "DXGID3D10CreateDevice",
    "DXGID3D10RegisterLayers",
    "DXGIGetDebugInterface1",
    "DXGIDeclareAdapterRemovalSupport",
];
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

thunk!(DXGID3D10CreateDevice, 3);
thunk!(DXGID3D10RegisterLayers, 4);
thunk!(DXGIGetDebugInterface1, 5);
thunk!(DXGIDeclareAdapterRemovalSupport, 6);

unsafe extern "system" {
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn OutputDebugStringA(s: *const u8);
}

pub(crate) fn debug(msg: &str) {
    let mut s = Vec::with_capacity(msg.len() + 32);
    s.extend_from_slice(b"[grace dxgi] ");
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
    path.extend("\\dxgi.dll\0".encode_utf16());
    // SAFETY: путь нуль-терминирован; builtin dxgi (D3DMetal), полный путь отличается от нашего.
    let real = unsafe { LoadLibraryW(path.as_ptr()) };
    let mut found = 0;
    for (slot, name) in REAL.iter().zip(NAMES) {
        let cname: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
        // SAFETY: модуль загружен или null.
        let addr = if real.is_null() { std::ptr::null_mut() } else { unsafe { GetProcAddress(real, cname.as_ptr()) } };
        found += usize::from(!addr.is_null());
        slot.store(addr as usize, Ordering::Release);
    }
    debug(&format!("загружен поверх системного dxgi.dll, функций {found}/{}", NAMES.len()));
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_m: *mut c_void, reason: u32, _r: *mut c_void) -> i32 {
    if reason == 1 {
        resolve_real();
    }
    1
}

// ---- копии vtable ---------------------------------------------------------------------------

type QueryInterface = unsafe extern "system" fn(*mut c_void, *const [u8; 16], *mut *mut c_void) -> i32;

/// Копия vtable одного класса объектов (фабрика или swapchain).
struct VtblCopy {
    orig: AtomicUsize,
    clone: AtomicUsize,
}

impl VtblCopy {
    const fn new() -> Self {
        Self { orig: AtomicUsize::new(0), clone: AtomicUsize::new(0) }
    }

    /// При первом вызове копирует `slots` указателей vtable объекта и ставит хуки.
    /// Затем подставляет копию объекту, если у него исходная vtable.
    unsafe fn adopt(&self, obj: *mut c_void, slots: usize, hooks: &[(usize, usize, &AtomicUsize)]) {
        if obj.is_null() {
            return;
        }
        // SAFETY: первое поле COM-объекта — указатель на vtable.
        let vtbl = unsafe { *(obj as *const *const usize) };
        if self.orig.load(Ordering::Acquire) == 0 {
            let mut copy = vec![0usize; slots].into_boxed_slice();
            // SAFETY: читаем `slots` указателей vtable (константные данные фреймворка).
            unsafe { std::ptr::copy_nonoverlapping(vtbl, copy.as_mut_ptr(), slots) };
            for (slot, hook, orig) in hooks {
                orig.store(copy[*slot], Ordering::Release);
                copy[*slot] = *hook;
            }
            self.clone.store(Box::leak(copy).as_ptr() as usize, Ordering::Release);
            self.orig.store(vtbl as usize, Ordering::Release);
        }
        if vtbl as usize == self.orig.load(Ordering::Acquire) {
            // SAFETY: объект в куче, первое поле доступно на запись.
            unsafe { std::ptr::write_volatile(obj as *mut usize, self.clone.load(Ordering::Acquire)) };
        }
    }
}

// Фабрика: IUnknown(0–2), IDXGIObject(3–6), IDXGIFactory: CreateSwapChain(10),
// IDXGIFactory2: CreateSwapChainForHwnd(15). Копируем 64 слота (до IDXGIFactory7 с запасом).
const FACTORY_SLOTS: usize = 64;
static FACTORY: VtblCopy = VtblCopy::new();
static ORIG_FACTORY_QI: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_SC: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_SC_HWND: AtomicUsize = AtomicUsize::new(0);

// Swapchain: IDXGIDeviceSubObject(7), Present(8), IDXGISwapChain1::Present1(22).
const SWAPCHAIN_SLOTS: usize = 64;
static SWAPCHAIN: VtblCopy = VtblCopy::new();
static ORIG_SC_QI: AtomicUsize = AtomicUsize::new(0);
static ORIG_PRESENT: AtomicUsize = AtomicUsize::new(0);
static ORIG_PRESENT1: AtomicUsize = AtomicUsize::new(0);

unsafe fn adopt_factory(obj: *mut c_void) {
    unsafe {
        FACTORY.adopt(
            obj,
            FACTORY_SLOTS,
            &[
                (0, factory_qi as *const () as usize, &ORIG_FACTORY_QI),
                (10, create_swapchain as *const () as usize, &ORIG_CREATE_SC),
                (15, create_swapchain_hwnd as *const () as usize, &ORIG_CREATE_SC_HWND),
            ],
        )
    };
}

unsafe fn adopt_swapchain(obj: *mut c_void) {
    let first = SWAPCHAIN.orig.load(Ordering::Acquire) == 0;
    unsafe {
        SWAPCHAIN.adopt(
            obj,
            SWAPCHAIN_SLOTS,
            &[
                (0, swapchain_qi as *const () as usize, &ORIG_SC_QI),
                (8, present as *const () as usize, &ORIG_PRESENT),
                (22, present1 as *const () as usize, &ORIG_PRESENT1),
            ],
        )
    };
    if first {
        debug("swapchain перехвачен: Present/Present1 считают время кадра");
        frames::start();
    }
}

// ---- фабрика ---------------------------------------------------------------------------------

type CreateFactory = unsafe extern "system" fn(*const [u8; 16], *mut *mut c_void) -> i32;
type CreateFactory2 = unsafe extern "system" fn(u32, *const [u8; 16], *mut *mut c_void) -> i32;

unsafe fn after_factory(hr: i32, out: *mut *mut c_void) -> i32 {
    static N: AtomicUsize = AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 6 {
        debug(&format!("фабрика/QI → {hr:#x}"));
    }
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — объект фабрики.
        unsafe { adopt_factory(*out) };
    }
    hr
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn CreateDXGIFactory(riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    // SAFETY: адрес из системного dxgi.dll.
    let real: CreateFactory = unsafe { std::mem::transmute(REAL[0].load(Ordering::Acquire)) };
    unsafe { after_factory(real(riid, out), out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn CreateDXGIFactory1(riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    // SAFETY: адрес из системного dxgi.dll.
    let real: CreateFactory = unsafe { std::mem::transmute(REAL[1].load(Ordering::Acquire)) };
    unsafe { after_factory(real(riid, out), out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn CreateDXGIFactory2(flags: u32, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    // SAFETY: адрес из системного dxgi.dll.
    let real: CreateFactory2 = unsafe { std::mem::transmute(REAL[2].load(Ordering::Acquire)) };
    unsafe { after_factory(real(flags, riid, out), out) }
}

unsafe extern "system" fn factory_qi(this: *mut c_void, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    // SAFETY: оригинал записан при копировании vtable.
    let orig: QueryInterface = unsafe { std::mem::transmute(ORIG_FACTORY_QI.load(Ordering::Acquire)) };
    unsafe { after_factory(orig(this, riid, out), out) }
}

unsafe fn after_swapchain(hr: i32, out: *mut *mut c_void) -> i32 {
    static N: AtomicUsize = AtomicUsize::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 6 {
        debug(&format!("swapchain/QI → {hr:#x}"));
    }
    if hr >= 0 && !out.is_null() {
        // SAFETY: при успехе *out — объект swapchain.
        unsafe { adopt_swapchain(*out) };
    }
    hr
}

unsafe extern "system" fn create_swapchain(this: *mut c_void, device: *mut c_void, desc: *mut c_void, out: *mut *mut c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *mut *mut c_void) -> i32;
    // SAFETY: оригинал записан при копировании vtable.
    let orig: F = unsafe { std::mem::transmute(ORIG_CREATE_SC.load(Ordering::Acquire)) };
    unsafe { after_swapchain(orig(this, device, desc, out), out) }
}

unsafe extern "system" fn create_swapchain_hwnd(
    this: *mut c_void,
    device: *mut c_void,
    hwnd: *mut c_void,
    desc: *const c_void,
    fs_desc: *const c_void,
    output: *mut c_void,
    out: *mut *mut c_void,
) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const c_void, *const c_void, *mut c_void, *mut *mut c_void) -> i32;
    // SAFETY: оригинал записан при копировании vtable.
    let orig: F = unsafe { std::mem::transmute(ORIG_CREATE_SC_HWND.load(Ordering::Acquire)) };
    unsafe { after_swapchain(orig(this, device, hwnd, desc, fs_desc, output, out), out) }
}

// ---- swapchain ------------------------------------------------------------------------------

unsafe extern "system" fn swapchain_qi(this: *mut c_void, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    // SAFETY: оригинал записан при копировании vtable.
    let orig: QueryInterface = unsafe { std::mem::transmute(ORIG_SC_QI.load(Ordering::Acquire)) };
    unsafe { after_swapchain(orig(this, riid, out), out) }
}

unsafe extern "system" fn present(this: *mut c_void, sync: u32, flags: u32) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, u32, u32) -> i32;
    frames::on_present(flags);
    // SAFETY: оригинал записан при копировании vtable.
    let orig: F = unsafe { std::mem::transmute(ORIG_PRESENT.load(Ordering::Acquire)) };
    unsafe { orig(this, sync, flags) }
}

unsafe extern "system" fn present1(this: *mut c_void, sync: u32, flags: u32, params: *const c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> i32;
    frames::on_present(flags);
    // SAFETY: оригинал записан при копировании vtable.
    let orig: F = unsafe { std::mem::transmute(ORIG_PRESENT1.load(Ordering::Acquire)) };
    unsafe { orig(this, sync, flags, params) }
}
