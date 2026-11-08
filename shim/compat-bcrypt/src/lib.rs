//! Compat shim `bcrypt.dll` for GPTK's Wine 7.7.
//!
//! Why: modern Windows programs (the Steam client, `tier0_s64.dll`,
//! `steamclient64.dll`) pass algorithm pseudo-handles to `BCryptGenRandom` and other functions
//! (`BCRYPT_RNG_ALG_HANDLE` = 0x81 etc., Windows 10+).
//! Wine 7.7 does not know them and dereferences them as pointers: the process crashes.
//!
//! How: the DLL exports every function of the real `bcrypt.dll`. Every export is
//! a naked trampoline with a `jmp` to the real function in Wine's builtin `bcrypt.dll`.
//! For functions whose first argument is an algorithm handle the trampoline first
//! replaces the pseudo-handle with a real one: the provider is opened via
//! `BCryptOpenAlgorithmProvider` once and cached.
//!
//! Setup: put it next to the exe as `bcrypt.dll` and enable for that exe
//! `DllOverrides: bcrypt=native,builtin` (see compat shims in `docs/ARCHITECTURE.md`).

#![cfg(all(windows, target_arch = "x86_64"))]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

macro_rules! thunk {
    (plain, $name:ident, $idx:expr) => {
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
    (alg_trampoline, $name:ident, $idx:expr) => {
        #[unsafe(no_mangle)]
        #[unsafe(naked)]
        pub unsafe extern "system" fn $name() {
            // Pseudo-handles are small numbers with the low bit set (0x01, 0x11, …, 0x81, …);
            // real handles are aligned pointers. Arguments 2-4 (rdx, r8, r9)
            // are preserved, stack arguments are left alone.
            core::arch::naked_asm!(
                "test cl, 1",
                "jz 2f",
                "cmp rcx, 0x1000",
                "jae 2f",
                "push rdx",
                "push r8",
                "push r9",
                // On entry rsp ≡ 8 (mod 16); three pushes and 32 bytes of shadow space give 0 (mod 16).
                "sub rsp, 32",
                "call {translate}",
                "add rsp, 32",
                "pop r9",
                "pop r8",
                "pop rdx",
                "mov rcx, rax",
                "2:",
                "jmp qword ptr [rip + {real} + {off}]",
                translate = sym crate::translate_pseudo,
                real = sym crate::REAL,
                off = const $idx * 8,
            );
        }
    };
}

mod getproc_fallback;
mod iat;
mod tcp_owners;
mod thunks;
mod winsock;
use thunks::NAMES;

const N: usize = NAMES.len();

/// Addresses of the real functions in `NAMES` order. Read from the asm trampolines.
static REAL: [AtomicUsize; N] = [const { AtomicUsize::new(0) }; N];

/// Cache of opened providers by pseudo-handle index (`h >> 4`).
static OPENED: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];

const STATUS_NOT_IMPLEMENTED: i32 = 0xC000_0002_u32 as i32;
const BCRYPT_ALG_HANDLE_HMAC_FLAG: u32 = 0x8;
const DLL_PROCESS_ATTACH: u32 = 1;

unsafe extern "system" {
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn OutputDebugStringA(s: *const u8);
}

pub(crate) fn debug(msg: &str) {
    let mut s = Vec::with_capacity(msg.len() + 32);
    s.extend_from_slice(b"[grace compat-bcrypt] ");
    s.extend_from_slice(msg.as_bytes());
    s.extend_from_slice(b"\n\0");
    // SAFETY: the string is null-terminated.
    unsafe { OutputDebugStringA(s.as_ptr()) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Pseudo-handles from bcrypt.h (Windows 10): value -> (algorithm id, HMAC?).
fn pseudo_alg(h: usize) -> Option<(&'static str, bool)> {
    Some(match h {
        0x01 => ("MD2", false),
        0x11 => ("MD4", false),
        0x21 => ("MD5", false),
        0x31 => ("SHA1", false),
        0x41 => ("SHA256", false),
        0x51 => ("SHA384", false),
        0x61 => ("SHA512", false),
        0x71 => ("RC4", false),
        0x81 => ("RNG", false),
        0x91 => ("MD5", true),
        0xa1 => ("SHA1", true),
        0xb1 => ("SHA256", true),
        0xc1 => ("SHA384", true),
        0xd1 => ("SHA512", true),
        0xe1 => ("RSA", false),
        0xf1 => ("ECDSA", false),
        _ => return None,
    })
}

fn open_index() -> usize {
    NAMES.iter().position(|n| *n == "BCryptOpenAlgorithmProvider").unwrap_or(0)
}

/// Called from the trampoline: pseudo-handle -> real provider handle.
/// On failure returns the original value: behavior as without the shim.
extern "system" fn translate_pseudo(h: usize) -> usize {
    let Some((alg, hmac)) = pseudo_alg(h) else {
        debug(&format!("unknown pseudo-handle {h:#x}, passing it through"));
        return h;
    };
    let slot = &OPENED[h >> 4];
    let cached = slot.load(Ordering::Acquire);
    if cached != 0 {
        return cached;
    }
    let open = REAL[open_index()].load(Ordering::Acquire);
    if open == 0 {
        return h;
    }
    // SAFETY: the address was obtained via GetProcAddress("BCryptOpenAlgorithmProvider").
    let open: extern "system" fn(*mut usize, *const u16, *const u16, u32) -> i32 =
        unsafe { std::mem::transmute::<usize, _>(open) };
    let mut handle = 0usize;
    let id = wide(alg);
    let flags = if hmac { BCRYPT_ALG_HANDLE_HMAC_FLAG } else { 0 };
    let status = open(&mut handle, id.as_ptr(), std::ptr::null(), flags);
    if status != 0 || handle == 0 {
        debug(&format!("provider {alg} (hmac={hmac}) did not open for {h:#x}: status {status:#x}"));
        return h;
    }
    // A race between two threads opens one extra provider: harmless.
    match slot.compare_exchange(0, handle, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            debug(&format!("pseudo-handle {h:#x} -> provider {alg} (hmac={hmac})"));
            handle
        }
        Err(existing) => existing,
    }
}

extern "system" fn not_implemented() -> i32 {
    STATUS_NOT_IMPLEMENTED
}

fn resolve_real() {
    let mut dir = [0u16; 260];
    // SAFETY: a 260-u16 buffer, its size is passed.
    let len = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
    let mut path: Vec<u16> = dir[..len.min(dir.len())].to_vec();
    path.extend(wide("\\bcrypt.dll"));
    // SAFETY: the path is null-terminated. This is Wine's builtin bcrypt: its full path differs
    // from ours, so the loader does not consider it already loaded.
    let real = unsafe { LoadLibraryW(path.as_ptr()) };
    if real.is_null() {
        debug("the system bcrypt.dll did not load: every call returns STATUS_NOT_IMPLEMENTED");
    }
    let mut resolved = 0;
    for (slot, name) in REAL.iter().zip(NAMES) {
        let mut cname = name.as_bytes().to_vec();
        cname.push(0);
        let addr = if real.is_null() {
            std::ptr::null_mut()
        } else {
            // SAFETY: the module is loaded, the name is null-terminated.
            unsafe { GetProcAddress(real, cname.as_ptr()) }
        };
        if addr.is_null() {
            slot.store(not_implemented as *const () as usize, Ordering::Release);
        } else {
            slot.store(addr as usize, Ordering::Release);
            resolved += 1;
        }
    }
    debug(&format!("loaded, real functions found {resolved}/{N}"));

    // Chromium looks up QueryUnbiasedInterruptTimePrecise via GetModuleHandle by the
    // API-set name, so the DLL must already be loaded. bcrypt.dll is imported by
    // steamwebhelper.exe, so we also load compat-realtime if it is installed.
    let mut rt: Vec<u16> = dir[..len.min(dir.len())].to_vec();
    rt.extend(wide("\\api-ms-win-core-realtime-l1-1-1.dll"));
    // SAFETY: the path is null-terminated; a missing file is not an error.
    let realtime = unsafe { LoadLibraryW(rt.as_ptr()) };
    if !realtime.is_null() {
        debug("compat-realtime loaded");
        getproc_fallback::install(realtime);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        resolve_real();
        // The other compat fixes: modules loaded later are patched at load time.
        iat::watch_loads();
        winsock::install();
        tcp_owners::install();
    }
    1
}
