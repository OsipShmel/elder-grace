//! Подмена записей таблицы импорта (IAT) и патчинг модулей в момент загрузки.
//!
//! Общий механизм compat-shim'ов: функция модуля-цели, импортированная из DLL Wine,
//! направляется в нашу обёртку. Ищем импорт по имени или по ординалу.
//! Модули, загружаемые позже нас, патчим из `LdrRegisterDllNotification`:
//! уведомление приходит после разрешения импортов, но до их DllMain.

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
    /// Модуль, чей IAT правим (имя файла, без учёта регистра); `"*"` — все модули.
    pub target: &'static str,
    /// Из какой DLL импорт (без учёта регистра); `None` — из любой.
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

/// Правит IAT модуля, загруженного по адресу `base`. Возвращает число слотов.
///
/// # Safety
/// `base` — адрес загруженного PE-образа x64.
pub unsafe fn patch_at(base: usize, p: &Patch) -> usize {
    let mut patched = 0;
    // SAFETY: разбираем заголовки загруженного образа по формату PE32+;
    // все смещения берутся из самого образа.
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
        // SAFETY: буфер на 1024 хендла, размер в байтах.
        if unsafe { K32EnumProcessModules(GetCurrentProcess(), mods.as_mut_ptr(), (mods.len() * 8) as u32, &mut needed) } == 0 {
            return;
        }
        for m in mods.iter().take(needed as usize / 8).filter(|m| !m.is_null()) {
            // SAFETY: хендл модуля = его база.
            let n = unsafe { patch_at(*m as usize, p) };
            if n > 0 {
                let mut name = [0u8; 260];
                // SAFETY: буфер на 260 байт.
                let len = unsafe { K32GetModuleBaseNameA(GetCurrentProcess(), *m, name.as_mut_ptr(), 260) } as usize;
                debug(&format!("{}: {} (слотов {n})", p.label, String::from_utf8_lossy(&name[..len.min(260)])));
            }
        }
        return;
    }
    let wname: Vec<u16> = p.target.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: имя нуль-терминировано.
    let base = unsafe { GetModuleHandleW(wname.as_ptr()) } as usize;
    if base != 0 {
        // SAFETY: GetModuleHandle вернул базу загруженного модуля.
        let n = unsafe { patch_at(base, p) };
        if n > 0 {
            debug(&format!("{}: {} (слотов {n})", p.label, p.target));
        }
    }
}

/// Регистрирует подмену: применяется сразу (если модуль уже загружен)
/// и к каждой будущей загрузке модуля с этим именем.
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
    // SAFETY: загрузчик передаёт валидную структуру и строку имени.
    let (name, base) = unsafe {
        let d = &*data;
        let s = &*d.base_name;
        let chars = std::slice::from_raw_parts(s.buf, usize::from(s.len) / 2);
        (String::from_utf16_lossy(chars), d.base as usize)
    };
    // Под loader lock: без загрузки DLL, только правка памяти.
    let Ok(list) = PATCHES.try_lock() else { return };
    for p in list.iter().filter(|p| p.target == "*" || name.eq_ignore_ascii_case(p.target)) {
        // SAFETY: base — база только что загруженного модуля.
        let n = unsafe { patch_at(base, p) };
        if n > 0 {
            debug(&format!("{}: {name} при загрузке (слотов {n})", p.label));
        }
    }
}

/// Включает патчинг модулей при загрузке. Вызывать один раз.
pub fn watch_loads() {
    type Register = unsafe extern "system" fn(u32, unsafe extern "system" fn(u32, *const NotificationData, *mut c_void), *mut c_void, *mut *mut c_void) -> i32;
    // SAFETY: ntdll загружен всегда, имя нуль-терминировано.
    let f = unsafe { GetProcAddress(GetModuleHandleA(c"ntdll.dll".as_ptr().cast()), c"LdrRegisterDllNotification".as_ptr().cast()) };
    if f.is_null() {
        debug("нет LdrRegisterDllNotification — модули, загруженные позже, не будут пропатчены");
        return;
    }
    // SAFETY: сигнатура из winternl/ntddk.
    let register: Register = unsafe { std::mem::transmute(f) };
    let mut cookie = std::ptr::null_mut();
    let status = unsafe { register(0, on_dll, std::ptr::null_mut(), &mut cookie) };
    if status != 0 {
        debug(&format!("LdrRegisterDllNotification: status {status:#x}"));
    }
}
