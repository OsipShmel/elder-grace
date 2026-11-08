//! Compat shim `api-ms-win-core-realtime-l1-1-1.dll` (and `-l1-1-2`) for GPTK's Wine 7.7.
//!
//! Why: Chromium/CEF (Steam's embedded browser) takes
//! `QueryUnbiasedInterruptTimePrecise` from this API set at startup and dies on a fatal
//! check if the function is missing. Wine 7.7 has neither the DLL nor the `*Precise` functions.
//!
//! How: our own implementations on top of what Wine has (`QueryUnbiasedInterruptTime`,
//! `QueryPerformanceCounter`); the rest is forwarded to kernel32 via GetProcAddress.
//! Installed into the prefix's `C:\windows\system32` under both API-set names:
//! CEF loads it with a search flag limited to system32.

#![cfg(all(windows, target_arch = "x86_64"))]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::sync::OnceLock;

type Bool = i32;
type HResult = i32;
const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;

unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn QueryPerformanceCounter(count: *mut i64) -> Bool;
    fn QueryPerformanceFrequency(freq: *mut i64) -> Bool;
    fn GetActiveProcessorCount(group: u16) -> u32;
}

fn kernel32(name: &'static [u8]) -> Option<*mut c_void> {
    // SAFETY: kernel32 is always loaded, names are null-terminated.
    let p = unsafe { GetProcAddress(GetModuleHandleA(c"kernel32.dll".as_ptr().cast()), name.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// QPC time in 100 ns units, like InterruptTime.
fn qpc_100ns() -> u64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: pointer to a local variable.
        unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    });
    let mut c = 0i64;
    // SAFETY: pointer to a local variable.
    unsafe { QueryPerformanceCounter(&mut c) };
    ((c as i128 * 10_000_000) / freq as i128) as u64
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn QueryUnbiasedInterruptTime(out: *mut u64) -> Bool {
    if out.is_null() {
        return 0;
    }
    if let Some(f) = kernel32(b"QueryUnbiasedInterruptTime\0") {
        // SAFETY: signature BOOL QueryUnbiasedInterruptTime(PULONGLONG).
        let f: extern "system" fn(*mut u64) -> Bool = unsafe { std::mem::transmute(f) };
        return f(out);
    }
    // SAFETY: out was checked for null.
    unsafe { *out = qpc_100ns() };
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn QueryUnbiasedInterruptTimePrecise(out: *mut u64) {
    // SAFETY: forwards the same pointer.
    unsafe { QueryUnbiasedInterruptTime(out) };
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn QueryInterruptTime(out: *mut u64) {
    if !out.is_null() {
        // SAFETY: out was checked for null.
        unsafe { *out = qpc_100ns() };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn QueryInterruptTimePrecise(out: *mut u64) {
    // SAFETY: forwards the same pointer.
    unsafe { QueryInterruptTime(out) };
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn QueryThreadCycleTime(thread: *mut c_void, out: *mut u64) -> Bool {
    match kernel32(b"QueryThreadCycleTime\0") {
        Some(f) => {
            // SAFETY: signature BOOL QueryThreadCycleTime(HANDLE, PULONG64).
            let f: extern "system" fn(*mut c_void, *mut u64) -> Bool = unsafe { std::mem::transmute(f) };
            f(thread, out)
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn QueryProcessCycleTime(process: *mut c_void, out: *mut u64) -> Bool {
    match kernel32(b"QueryProcessCycleTime\0") {
        Some(f) => {
            // SAFETY: signature BOOL QueryProcessCycleTime(HANDLE, PULONG64).
            let f: extern "system" fn(*mut c_void, *mut u64) -> Bool = unsafe { std::mem::transmute(f) };
            f(process, out)
        }
        None => 0,
    }
}

/// Wine does not track processor idle time: return zeros of the right length.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn QueryIdleProcessorCycleTimeEx(group: u16, len: *mut u32, buf: *mut u64) -> Bool {
    if len.is_null() {
        return 0;
    }
    // SAFETY: GetActiveProcessorCount is safe; pointers were checked.
    let n = unsafe { GetActiveProcessorCount(group) } as usize;
    let need = (n * 8) as u32;
    unsafe {
        if !buf.is_null() && *len >= need {
            std::ptr::write_bytes(buf, 0, n);
        }
        *len = need;
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn QueryIdleProcessorCycleTime(len: *mut u32, buf: *mut u64) -> Bool {
    // SAFETY: forwards the same pointers; ALL_PROCESSOR_GROUPS = 0xffff.
    unsafe { QueryIdleProcessorCycleTimeEx(0xffff, len, buf) }
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryAuxiliaryCounterFrequency(_freq: *mut u64) -> HResult {
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn ConvertAuxiliaryCounterToPerformanceCounter(_v: u64, _out: *mut u64, _err: *mut u64) -> HResult {
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn ConvertPerformanceCounterToAuxiliaryCounter(_v: u64, _out: *mut u64, _err: *mut u64) -> HResult {
    E_NOTIMPL
}
