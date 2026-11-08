//! Fallback `GetProcAddress` for modules that look up Windows 10+ functions.
//!
//! Chromium does `GetProcAddress(GetModuleHandle("api-ms-win-core-realtime-l1-1-1.dll"),
//! "QueryUnbiasedInterruptTimePrecise")`. Wine 7.7 resolves the API-set name to its own
//! `kernelbase`, which lacks the function, and gets NULL -> fatal check.
//!
//! The wrapper asks the real `GetProcAddress` first, and only when it returns
//! NULL looks for a function with the same name in the compat DLL (`compat-realtime`).
//! Behavior for every existing function stays the same.

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
    // SAFETY: the address of the real GetProcAddress was stored before the hook was installed.
    let real: GetProcFn = unsafe { std::mem::transmute(REAL_GETPROC.load(Ordering::Acquire)) };
    let found = unsafe { real(module, name) };
    // Import by ordinal (low word): not our case.
    if !found.is_null() || (name as usize) <= 0xFFFF {
        return found;
    }
    let fallback = FALLBACK_MODULE.load(Ordering::Acquire) as *mut c_void;
    if fallback.is_null() {
        return found;
    }
    let ours = unsafe { real(fallback, name) };
    if !ours.is_null() {
        // SAFETY: name is a null-terminated string from the caller.
        let n = unsafe { CStr::from_ptr(name.cast()) }.to_string_lossy();
        debug(&format!("GetProcAddress({n}) → compat"));
    }
    ours
}

/// Enables the fallback GetProcAddress if the compat DLL is loaded.
pub fn install(fallback_module: *mut c_void) {
    if fallback_module.is_null() {
        return;
    }
    // SAFETY: kernel32 is always loaded; take the real GetProcAddress before installing the hook.
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
            label: "fallback GetProcAddress",
        });
    }
}
