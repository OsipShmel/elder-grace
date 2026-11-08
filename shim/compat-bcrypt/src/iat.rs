//! Import address table (IAT) entry replacement and patching modules at load time.
//!
//! The common mechanism of the compat shims: a target module's function imported from a Wine DLL
//! is redirected to our wrapper. Imports are found by name or by ordinal.
//! Modules loaded after us are patched from `LdrRegisterDllNotification`:
//! the notification arrives after imports are resolved but before their DllMain.

use std::ffi::{CStr, c_void};
use std::sync::Mutex;

use crate::debug;

#[derive(Clone, Copy)]
pub enum Import {
    Name(&'static [u8]),
    Ordinal(u16),
}

#[derive(Clone, Copy)]
pub struct Patch {
    /// Module whose IAT is patched (file name, case-insensitive); `"*"` means every module.
    pub target: &'static str,
    /// DLL the import comes from (case-insensitive); `None` means any.
    pub from: Option<&'static str>,
    pub import: Import,
    pub hook: usize,
    pub label: &'static str,
}

unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn K32EnumProcessModules(process: *mut c_void, mods: *mut *mut c_void, cb: u32, needed: *mut u32) -> i32;
    fn K32GetModuleBaseNameA(process: *mut c_void, module: *mut c_void, name: *mut u8, size: u32) -> u32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
}

const PAGE_READWRITE: u32 = 0x04;

static PATCHES: Mutex<Vec<Patch>> = Mutex::new(Vec::new());

/// Patches the IAT of the module loaded at `base`. Returns the number of slots.
///
/// # Safety
/// `base` is the address of a loaded x64 PE image.
pub unsafe fn patch_at(base: usize, p: &Patch) -> usize {
    let mut patched = 0;
    // SAFETY: parses the headers of a loaded image per the PE32+ format;
    // every offset comes from the image itself.
    unsafe {
        let rd32 = |off: usize| std::ptr::read_unaligned((base + off) as *const u32) as usize;
        let nt = rd32(0x3c);
        let import_rva = rd32(nt + 24 + 120); // OptionalHeader.DataDirectory[1]
        if import_rva == 0 {
            return 0;
        }
        let mut desc = base + import_rva;
        loop {
            let original_first_thunk = std::ptr::read_unaligned(desc as *const u32) as usize;
            let name_rva = std::ptr::read_unaligned((desc + 12) as *const u32) as usize;
            let first_thunk = std::ptr::read_unaligned((desc + 16) as *const u32) as usize;
            if name_rva == 0 {
                break;
            }
            let dll = CStr::from_ptr((base + name_rva) as *const _).to_string_lossy();
            let dll_matches = p.from.is_none_or(|f| dll.eq_ignore_ascii_case(f));
            if dll_matches {
                let lookup = if original_first_thunk != 0 { original_first_thunk } else { first_thunk };
                let mut i = 0;
                loop {
                    let entry = std::ptr::read_unaligned((base + lookup + i * 8) as *const u64);
                    if entry == 0 {
                        break;
                    }
                    let hit = match p.import {
                        Import::Ordinal(o) => entry & (1 << 63) != 0 && (entry & 0xFFFF) as u16 == o,
                        Import::Name(n) => {
                            entry & (1 << 63) == 0
                                && CStr::from_ptr((base + entry as usize + 2) as *const _).to_bytes() == n
                        }
                    };
                    if hit {
                        let slot = (base + first_thunk + i * 8) as *mut usize;
                        let mut old = 0u32;
                        if VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old) != 0 {
                            std::ptr::write_volatile(slot, p.hook);
                            VirtualProtect(slot.cast(), 8, old, &mut old);
                            patched += 1;
                        }
                    }
                    i += 1;
                }
            }
            desc += 20;
        }
    }
    patched
}

fn apply_to_loaded(p: &Patch) {
    if p.target == "*" {
        let mut mods = vec![std::ptr::null_mut::<c_void>(); 1024];
        let mut needed = 0u32;
        // SAFETY: a buffer for 1024 handles, size in bytes.
        if unsafe { K32EnumProcessModules(GetCurrentProcess(), mods.as_mut_ptr(), (mods.len() * 8) as u32, &mut needed) } == 0 {
            return;
        }
        for m in mods.iter().take(needed as usize / 8).filter(|m| !m.is_null()) {
            // SAFETY: a module handle is its base.
            let n = unsafe { patch_at(*m as usize, p) };
            if n > 0 {
                let mut name = [0u8; 260];
                // SAFETY: a 260-byte buffer.
                let len = unsafe { K32GetModuleBaseNameA(GetCurrentProcess(), *m, name.as_mut_ptr(), 260) } as usize;
                debug(&format!("{}: {} ({n} slots)", p.label, String::from_utf8_lossy(&name[..len.min(260)])));
            }
        }
        return;
    }
    let wname: Vec<u16> = p.target.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the name is null-terminated.
    let base = unsafe { GetModuleHandleW(wname.as_ptr()) } as usize;
    if base != 0 {
        // SAFETY: GetModuleHandle returned the base of a loaded module.
        let n = unsafe { patch_at(base, p) };
        if n > 0 {
            debug(&format!("{}: {} ({n} slots)", p.label, p.target));
        }
    }
}

/// Registers a patch: applied right away (if the module is already loaded)
/// and to every future load of a module with that name.
pub fn register(p: Patch) {
    apply_to_loaded(&p);
    if let Ok(mut list) = PATCHES.lock() {
        list.push(p);
    }
}

#[repr(C)]
struct UnicodeString {
    len: u16,
    max: u16,
    buf: *const u16,
}

#[repr(C)]
struct NotificationData {
    flags: u32,
    full_name: *const UnicodeString,
    base_name: *const UnicodeString,
    base: *mut c_void,
    size: u32,
}

const LDR_DLL_NOTIFICATION_REASON_LOADED: u32 = 1;

unsafe extern "system" fn on_dll(reason: u32, data: *const NotificationData, _ctx: *mut c_void) {
    if reason != LDR_DLL_NOTIFICATION_REASON_LOADED || data.is_null() {
        return;
    }
    // SAFETY: the loader passes a valid structure and name string.
    let (name, base) = unsafe {
        let d = &*data;
        let s = &*d.base_name;
        let chars = std::slice::from_raw_parts(s.buf, usize::from(s.len) / 2);
        (String::from_utf16_lossy(chars), d.base as usize)
    };
    // Under the loader lock: no DLL loading, memory edits only.
    let Ok(list) = PATCHES.try_lock() else { return };
    for p in list.iter().filter(|p| p.target == "*" || name.eq_ignore_ascii_case(p.target)) {
        // SAFETY: base is the base of a just-loaded module.
        let n = unsafe { patch_at(base, p) };
        if n > 0 {
            debug(&format!("{}: {name} on load ({n} slots)", p.label));
        }
    }
}

/// Enables patching modules at load time. Call once.
pub fn watch_loads() {
    type Register = unsafe extern "system" fn(u32, unsafe extern "system" fn(u32, *const NotificationData, *mut c_void), *mut c_void, *mut *mut c_void) -> i32;
    // SAFETY: ntdll is always loaded, the name is null-terminated.
    let f = unsafe { GetProcAddress(GetModuleHandleA(c"ntdll.dll".as_ptr().cast()), c"LdrRegisterDllNotification".as_ptr().cast()) };
    if f.is_null() {
        debug("no LdrRegisterDllNotification: modules loaded later will not be patched");
        return;
    }
    // SAFETY: signature from winternl/ntddk.
    let register: Register = unsafe { std::mem::transmute(f) };
    let mut cookie = std::ptr::null_mut();
    let status = unsafe { register(0, on_dll, std::ptr::null_mut(), &mut cookie) };
    if status != 0 {
        debug(&format!("LdrRegisterDllNotification: status {status:#x}"));
    }
}
