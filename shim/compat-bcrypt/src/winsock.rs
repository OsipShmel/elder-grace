//! `getsockname` на сокете, принятом через `AcceptEx`.
//!
//! На Windows после `AcceptEx` программа вызывает
//! `setsockopt(accepted, SOL_SOCKET, SO_UPDATE_ACCEPT_CONTEXT, &listening)`,
//! и принятый сокет наследует свойства слушающего: локальный адрес в том числе.
//! Wine 7.7 этот вызов игнорирует («since our sockets are normal»), а `getsockname`
//! на принятом сокете возвращает `WSAEINVAL`. Клиент Steam считает транспорт
//! с `steamwebhelper` сломанным → «Непредвиденная ошибка транспорта (0x3007)».
//!
//! Обёртки запоминают пару «принятый → слушающий» и, если Wine не отдаёт локальный
//! адрес принятого сокета, отдают адрес слушающего — как на Windows.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::debug;
use crate::iat::{self, Import, Patch};

type Socket = usize;
type GetSockName = unsafe extern "system" fn(Socket, *mut c_void, *mut i32) -> i32;
type SetSockOpt = unsafe extern "system" fn(Socket, i32, i32, *const u8, i32) -> i32;
type CloseSocket = unsafe extern "system" fn(Socket) -> i32;

const SOL_SOCKET: i32 = 0xFFFF;
const SO_UPDATE_ACCEPT_CONTEXT: i32 = 0x700B;
const WSAEINVAL: i32 = 10022;

// Ординалы ws2_32: клиент Steam импортирует их не по имени.
const ORD_CLOSESOCKET: u16 = 3;
const ORD_GETSOCKNAME: u16 = 6;
const ORD_SETSOCKOPT: u16 = 21;

static REAL_GETSOCKNAME: AtomicUsize = AtomicUsize::new(0);
static REAL_SETSOCKOPT: AtomicUsize = AtomicUsize::new(0);
static REAL_CLOSESOCKET: AtomicUsize = AtomicUsize::new(0);
static ACCEPTED_FROM: Mutex<Option<HashMap<Socket, Socket>>> = Mutex::new(None);
static REAL_WSAIOCTL: AtomicUsize = AtomicUsize::new(0);
static REAL_ACCEPTEX: AtomicUsize = AtomicUsize::new(0);

type WsaIoctl = unsafe extern "system" fn(Socket, u32, *const u8, u32, *mut c_void, u32, *mut u32, *mut c_void, *mut c_void) -> i32;
type AcceptEx = unsafe extern "system" fn(Socket, Socket, *mut c_void, u32, u32, u32, *mut u32, *mut c_void) -> i32;

const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xC800_0006;
/// WSAID_ACCEPTEX {b5367df1-cbac-11cf-95ca-00805f48a192} в байтовом представлении GUID.
const WSAID_ACCEPTEX: [u8; 16] = [0xf1, 0x7d, 0x36, 0xb5, 0xac, 0xcb, 0xcf, 0x11, 0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92];

unsafe extern "system" fn hooked_acceptex(
    listen: Socket,
    accept: Socket,
    buf: *mut c_void,
    recv_len: u32,
    local_len: u32,
    remote_len: u32,
    bytes: *mut u32,
    overlapped: *mut c_void,
) -> i32 {
    if let Ok(mut map) = ACCEPTED_FROM.lock() {
        map.get_or_insert_with(HashMap::new).insert(accept, listen);
    }
    // SAFETY: адрес настоящего AcceptEx записан до того, как отдали обёртку.
    let real: AcceptEx = unsafe { std::mem::transmute(REAL_ACCEPTEX.load(Ordering::Acquire)) };
    unsafe { real(listen, accept, buf, recv_len, local_len, remote_len, bytes, overlapped) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn hooked_wsaioctl(
    s: Socket,
    code: u32,
    inbuf: *const u8,
    inlen: u32,
    outbuf: *mut c_void,
    outlen: u32,
    bytes: *mut u32,
    overlapped: *mut c_void,
    completion: *mut c_void,
) -> i32 {
    // SAFETY: адрес настоящей функции записан до установки хука.
    let real: WsaIoctl = unsafe { std::mem::transmute(REAL_WSAIOCTL.load(Ordering::Acquire)) };
    let r = unsafe { real(s, code, inbuf, inlen, outbuf, outlen, bytes, overlapped, completion) };
    if r == 0
        && code == SIO_GET_EXTENSION_FUNCTION_POINTER
        && inlen == 16
        && !inbuf.is_null()
        && outlen as usize >= size_of::<usize>()
        && !outbuf.is_null()
        // SAFETY: inbuf — GUID на 16 байт по контракту SIO_GET_EXTENSION_FUNCTION_POINTER.
        && unsafe { std::slice::from_raw_parts(inbuf, 16) } == WSAID_ACCEPTEX
    {
        let out = outbuf.cast::<usize>();
        // SAFETY: outbuf — место под указатель на функцию.
        let ptr = unsafe { std::ptr::read_unaligned(out) };
        if ptr != 0 && ptr != hooked_acceptex as *const () as usize {
            REAL_ACCEPTEX.store(ptr, Ordering::Release);
            unsafe { std::ptr::write_unaligned(out, hooked_acceptex as *const () as usize) };
            debug("AcceptEx отдан через обёртку");
        }
    }
    r
}

unsafe extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn WSAGetLastError() -> i32;
    fn WSASetLastError(err: i32);
}

unsafe extern "system" fn hooked_setsockopt(s: Socket, level: i32, name: i32, val: *const u8, len: i32) -> i32 {
    if level == SOL_SOCKET && name == SO_UPDATE_ACCEPT_CONTEXT && !val.is_null() && len as usize >= size_of::<Socket>() {
        // SAFETY: по контракту SO_UPDATE_ACCEPT_CONTEXT val указывает на SOCKET слушающего сокета.
        let listening = unsafe { std::ptr::read_unaligned(val.cast::<Socket>()) };
        // Steam передаёт сюда сам принятый сокет — такую пару не записываем,
        // настоящая берётся из AcceptEx.
        if listening != s
            && let Ok(mut map) = ACCEPTED_FROM.lock()
        {
            map.get_or_insert_with(HashMap::new).insert(s, listening);
        }
    }
    // SAFETY: адрес настоящей функции записан до установки хука.
    let real: SetSockOpt = unsafe { std::mem::transmute(REAL_SETSOCKOPT.load(Ordering::Acquire)) };
    unsafe { real(s, level, name, val, len) }
}

unsafe extern "system" fn hooked_getsockname(s: Socket, addr: *mut c_void, len: *mut i32) -> i32 {
    // SAFETY: адрес настоящей функции записан до установки хука.
    let real: GetSockName = unsafe { std::mem::transmute(REAL_GETSOCKNAME.load(Ordering::Acquire)) };
    let r = unsafe { real(s, addr, len) };
    let err = if r == 0 { 0 } else { unsafe { WSAGetLastError() } };
    if r == 0 || err != WSAEINVAL {
        return r;
    }
    let listening = ACCEPTED_FROM.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&s).copied()));
    let Some(listening) = listening else {
        debug(&format!("getsockname({s:#x}) → WSAEINVAL, слушающий сокет неизвестен"));
        unsafe { WSASetLastError(WSAEINVAL) };
        return r;
    };
    let r2 = unsafe { real(listening, addr, len) };
    if r2 == 0 {
        debug(&format!("getsockname({s:#x}) → адрес слушающего {listening:#x}"));
        unsafe { WSASetLastError(0) };
        return 0;
    }
    unsafe { WSASetLastError(WSAEINVAL) };
    r
}

unsafe extern "system" fn hooked_closesocket(s: Socket) -> i32 {
    if let Ok(mut map) = ACCEPTED_FROM.lock()
        && let Some(m) = map.as_mut()
    {
        m.remove(&s);
    }
    crate::tcp_owners::on_close(s);
    // SAFETY: адрес настоящей функции записан до установки хука.
    let real: CloseSocket = unsafe { std::mem::transmute(REAL_CLOSESOCKET.load(Ordering::Acquire)) };
    unsafe { real(s) }
}

pub fn install() {
    // SAFETY: ws2_32 — системная DLL Wine; имена нуль-терминированы.
    let ws2 = unsafe { LoadLibraryA(c"ws2_32.dll".as_ptr().cast()) };
    if ws2.is_null() {
        return;
    }
    let get = |n: &std::ffi::CStr| unsafe { GetProcAddress(ws2, n.as_ptr().cast()) } as usize;
    let (gsn, sso, cls, ioc) = (get(c"getsockname"), get(c"setsockopt"), get(c"closesocket"), get(c"WSAIoctl"));
    if gsn == 0 || sso == 0 || cls == 0 || ioc == 0 {
        return;
    }
    REAL_WSAIOCTL.store(ioc, Ordering::Release);
    iat::register(Patch {
        target: "*",
        from: Some("ws2_32.dll"),
        import: Import::Name(b"WSAIoctl"),
        hook: hooked_wsaioctl as *const () as usize,
        label: "winsock WSAIoctl",
    });
    REAL_GETSOCKNAME.store(gsn, Ordering::Release);
    REAL_SETSOCKOPT.store(sso, Ordering::Release);
    REAL_CLOSESOCKET.store(cls, Ordering::Release);

    for (ord, hook, label) in [
        (ORD_GETSOCKNAME, hooked_getsockname as *const () as usize, "winsock getsockname"),
        (ORD_SETSOCKOPT, hooked_setsockopt as *const () as usize, "winsock setsockopt"),
        (ORD_CLOSESOCKET, hooked_closesocket as *const () as usize, "winsock closesocket"),
    ] {
        iat::register(Patch { target: "*", from: Some("ws2_32.dll"), import: Import::Ordinal(ord), hook, label });
    }
}
