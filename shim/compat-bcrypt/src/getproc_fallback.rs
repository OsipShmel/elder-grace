//! Запасной `GetProcAddress` для модулей, которые ищут функции Windows 10+.
//!
//! Chromium делает `GetProcAddress(GetModuleHandle("api-ms-win-core-realtime-l1-1-1.dll"),
//! "QueryUnbiasedInterruptTimePrecise")`. Wine 7.7 разрешает имя API-set в свой
//! `kernelbase`, где функции нет, и получает NULL → фатальная проверка.
//!
//! Обёртка сначала спрашивает настоящий `GetProcAddress`, и только если тот вернул
//! NULL — ищет функцию с тем же именем в compat-DLL (`compat-realtime`).
//! Для всех существующих функций поведение не меняется.

use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::debug;
use crate::iat::{self, Import, Patch};

type GetProcFn = unsafe extern "system" fn(*mut c_void, *const u8) -> *mut c_void;

static REAL_GETPROC: AtomicUsize = AtomicUsize::new(0);
static FALLBACK_MODULE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

unsafe extern "system" fn hooked_get_proc(module: *mut c_void, name: *const u8) -> *mut c_void {
    // SAFETY: адрес настоящего GetProcAddress записан до установки хука.
    let real: GetProcFn = unsafe { std::mem::transmute(REAL_GETPROC.load(Ordering::Acquire)) };
    let found = unsafe { real(module, name) };
    // Импорт по ординалу (младшее слово) — не наш случай.
    if !found.is_null() || (name as usize) <= 0xFFFF {
        return found;
    }
    let fallback = FALLBACK_MODULE.load(Ordering::Acquire) as *mut c_void;
    if fallback.is_null() {
        return found;
    }
    let ours = unsafe { real(fallback, name) };
    if !ours.is_null() {
        // SAFETY: name — нуль-терминированная строка от вызывающего.
        let n = unsafe { CStr::from_ptr(name.cast()) }.to_string_lossy();
        debug(&format!("GetProcAddress({n}) → compat"));
    }
    ours
}

/// Включает запасной GetProcAddress, если загружена compat-DLL.
pub fn install(fallback_module: *mut c_void) {
    if fallback_module.is_null() {
        return;
    }
    // SAFETY: kernel32 загружен всегда; берём настоящий GetProcAddress до установки хука.
    let real = unsafe { GetProcAddress(GetModuleHandleA(c"kernel32.dll".as_ptr().cast()), c"GetProcAddress".as_ptr().cast()) };
    if real.is_null() {
        return;
    }
    REAL_GETPROC.store(real as usize, Ordering::Release);
    FALLBACK_MODULE.store(fallback_module as usize, Ordering::Release);
    for target in ["libcef.dll", "steamwebhelper.exe", "chrome_elf.dll"] {
        iat::register(Patch {
            target,
            from: None,
            import: Import::Name(b"GetProcAddress"),
            hook: hooked_get_proc as *const () as usize,
            label: "запасной GetProcAddress",
        });
    }
}
