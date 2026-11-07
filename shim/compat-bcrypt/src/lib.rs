//! Compat-shim `bcrypt.dll` для Wine 7.7 из GPTK.
//!
//! Зачем: современные Windows-программы (клиент Steam, `tier0_s64.dll`,
//! `steamclient64.dll`) передают в `BCryptGenRandom` и другие функции
//! псевдо-хендлы алгоритмов (`BCRYPT_RNG_ALG_HANDLE` = 0x81 и т.п., Windows 10+).
//! Wine 7.7 их не знает и разыменовывает как указатель — процесс падает.
//!
//! Как: DLL экспортирует все функции настоящего `bcrypt.dll`. Каждый экспорт —
//! naked-трамплин с `jmp` на настоящую функцию из builtin `bcrypt.dll` Wine.
//! Функциям, у которых первый аргумент — хендл алгоритма, трамплин сначала
//! подменяет псевдо-хендл на настоящий: провайдер открывается через
//! `BCryptOpenAlgorithmProvider` один раз и кэшируется.
//!
//! Подключение: положить как `bcrypt.dll` рядом с exe и включить для этого exe
//! `DllOverrides: bcrypt=native,builtin` (см. compat-shim'ы в `docs/ARCHITECTURE.md`).

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
            // Псевдо-хендлы — маленькие числа с младшим битом 1 (0x01, 0x11, …, 0x81, …);
            // настоящие хендлы — выровненные указатели. Аргументы 2–4 (rdx, r8, r9)
            // сохраняем, стековые аргументы не трогаем.
            core::arch::naked_asm!(
                "test cl, 1",
                "jz 2f",
                "cmp rcx, 0x1000",
                "jae 2f",
                "push rdx",
                "push r8",
                "push r9",
                // На входе rsp ≡ 8 (mod 16); три push и 32 байта shadow space дают 0 (mod 16).
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

/// Адреса настоящих функций в порядке `NAMES`. Читается из asm-трамплинов.
static REAL: [AtomicUsize; N] = [const { AtomicUsize::new(0) }; N];

/// Кэш открытых провайдеров по индексу псевдо-хендла (`h >> 4`).
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
    // SAFETY: строка нуль-терминирована.
    unsafe { OutputDebugStringA(s.as_ptr()) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Псевдо-хендлы из bcrypt.h (Windows 10): значение → (идентификатор алгоритма, HMAC?).
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

/// Вызывается из трамплина: псевдо-хендл → настоящий хендл провайдера.
/// При неудаче возвращает исходное значение — поведение как без shim.
extern "system" fn translate_pseudo(h: usize) -> usize {
    let Some((alg, hmac)) = pseudo_alg(h) else {
        debug(&format!("неизвестный псевдо-хендл {h:#x}, пропускаю как есть"));
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
    // SAFETY: адрес получен через GetProcAddress("BCryptOpenAlgorithmProvider").
    let open: extern "system" fn(*mut usize, *const u16, *const u16, u32) -> i32 =
        unsafe { std::mem::transmute::<usize, _>(open) };
    let mut handle = 0usize;
    let id = wide(alg);
    let flags = if hmac { BCRYPT_ALG_HANDLE_HMAC_FLAG } else { 0 };
    let status = open(&mut handle, id.as_ptr(), std::ptr::null(), flags);
    if status != 0 || handle == 0 {
        debug(&format!("не открылся провайдер {alg} (hmac={hmac}) для {h:#x}: status {status:#x}"));
        return h;
    }
    // Гонка двух потоков даст один лишний открытый провайдер — некритично.
    match slot.compare_exchange(0, handle, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            debug(&format!("псевдо-хендл {h:#x} → провайдер {alg} (hmac={hmac})"));
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
    // SAFETY: буфер на 260 u16, размер передан.
    let len = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
    let mut path: Vec<u16> = dir[..len.min(dir.len())].to_vec();
    path.extend(wide("\\bcrypt.dll"));
    // SAFETY: путь нуль-терминирован. Это builtin bcrypt Wine: полный путь отличается
    // от нашего, поэтому загрузчик не считает его уже загруженным.
    let real = unsafe { LoadLibraryW(path.as_ptr()) };
    if real.is_null() {
        debug("не загрузился системный bcrypt.dll — все вызовы вернут STATUS_NOT_IMPLEMENTED");
    }
    let mut resolved = 0;
    for (slot, name) in REAL.iter().zip(NAMES) {
        let mut cname = name.as_bytes().to_vec();
        cname.push(0);
        let addr = if real.is_null() {
            std::ptr::null_mut()
        } else {
            // SAFETY: модуль загружен, имя нуль-терминировано.
            unsafe { GetProcAddress(real, cname.as_ptr()) }
        };
        if addr.is_null() {
            slot.store(not_implemented as *const () as usize, Ordering::Release);
        } else {
            slot.store(addr as usize, Ordering::Release);
            resolved += 1;
        }
    }
    debug(&format!("загружен, настоящих функций найдено {resolved}/{N}"));

    // Chromium ищет QueryUnbiasedInterruptTimePrecise через GetModuleHandle по имени
    // API-set, то есть DLL должна быть уже загружена. bcrypt.dll импортируется
    // steamwebhelper.exe, поэтому заодно подгружаем compat-realtime, если он установлен.
    let mut rt: Vec<u16> = dir[..len.min(dir.len())].to_vec();
    rt.extend(wide("\\api-ms-win-core-realtime-l1-1-1.dll"));
    // SAFETY: путь нуль-терминирован; отсутствие файла — не ошибка.
    let realtime = unsafe { LoadLibraryW(rt.as_ptr()) };
    if !realtime.is_null() {
        debug("подгружен compat-realtime");
        getproc_fallback::install(realtime);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        resolve_real();
        // Остальные compat-исправления: модули, загруженные позже, патчим при загрузке.
        iat::watch_loads();
        winsock::install();
        tcp_owners::install();
    }
    1
}
