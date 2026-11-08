//! TCP connection table with owner PIDs (`GetExtendedTcpTable`).
//!
//! Wine 7.7 on macOS returns an empty TCP table. The Steam client uses it to check
//! that the websocket to it was opened by its own `steamwebhelper` (it looks for the connection row
//! and its `owningPid`), does not find it and rejects the connection ->
//! "Unexpected transport error (0x3009)".
//!
//! A registry in prefix-wide shared memory: every process with our shim records its
//! `connect` and `listen` calls (addresses, ports, PID). The hooked `GetExtendedTcpTable`
//! appends rows from the registry to Wine's answer for the requested table class.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::debug;
use crate::iat::{self, Import, Patch};

type Socket = usize;

const SLOTS: usize = 1024;
const MAGIC: u32 = 0x4752_5443; // "GRTC"
const STATE_LISTEN: u32 = 2;
const STATE_ESTAB: u32 = 5;
const AF_INET: u32 = 2;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const NO_ERROR: u32 = 0;

#[repr(C)]
struct Entry {
    /// 0 = free, 1 = taken (changed atomically, memory shared between processes).
    used: AtomicU32,
    pid: u32,
    socket: u64,
    state: u32,
    local_addr: u32,  // as in sockaddr_in (network order)
    local_port: u32,  // network order in the low 16 bits
    remote_addr: u32,
    remote_port: u32,
}

#[repr(C)]
struct Table {
    magic: u32,
    _pad: u32,
    entries: [Entry; SLOTS],
}

unsafe extern "system" {
    fn CreateFileMappingW(file: *mut c_void, attrs: *mut c_void, protect: u32, hi: u32, lo: u32, name: *const u16) -> *mut c_void;
    fn MapViewOfFile(map: *mut c_void, access: u32, hi: u32, lo: u32, size: usize) -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn WSAGetLastError() -> i32;
    fn WSASetLastError(err: i32);
}

fn table() -> Option<&'static Table> {
    static TABLE: OnceLock<usize> = OnceLock::new();
    let addr = *TABLE.get_or_init(|| {
        let name: Vec<u16> = "Local\\ElderGraceTcpOwners".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: an anonymous (page-file backed) named mapping of the right size.
        unsafe {
            let map = CreateFileMappingW((-1isize) as *mut c_void, std::ptr::null_mut(), 0x04, 0, size_of::<Table>() as u32, name.as_ptr());
            if map.is_null() {
                return 0;
            }
            let view = MapViewOfFile(map, 0x0002 | 0x0004, 0, 0, size_of::<Table>());
            if view.is_null() {
                return 0;
            }
            let t = &mut *view.cast::<Table>();
            // A new mapping is zero-filled; the first process sets the magic.
            if t.magic != MAGIC {
                t.magic = MAGIC;
            }
            view as usize
        }
    });
    // SAFETY: the address of a live mapping for the whole process lifetime.
    (addr != 0).then(|| unsafe { &*(addr as *const Table) })
}

fn record(socket: Socket, state: u32, local: (u32, u32), remote: (u32, u32)) {
    let Some(t) = table() else { return };
    // SAFETY: no arguments.
    let pid = unsafe { GetCurrentProcessId() };
    for e in &t.entries {
        if e.used.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            // Slot taken (2 = being filled): write the fields and publish.
            // SAFETY: the slot is ours while used == 2; other readers skip it.
            unsafe {
                let p = std::ptr::from_ref(e).cast_mut();
                (*p).pid = pid;
                (*p).socket = socket as u64;
                (*p).state = state;
                (*p).local_addr = local.0;
                (*p).local_port = local.1;
                (*p).remote_addr = remote.0;
                (*p).remote_port = remote.1;
            }
            e.used.store(1, Ordering::Release);
            return;
        }
    }
}

fn forget(socket: Socket) {
    let Some(t) = table() else { return };
    // SAFETY: no arguments.
    let pid = unsafe { GetCurrentProcessId() };
    for e in &t.entries {
        if e.used.load(Ordering::Acquire) == 1 && e.pid == pid && e.socket == socket as u64 {
            e.used.store(0, Ordering::Release);
        }
    }
}

fn alive(pid: u32) -> bool {
    // SAFETY: OpenProcess with minimal rights; the handle is closed right away.
    unsafe {
        let h = OpenProcess(0x1000, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
    }
    true
}

/// (address, port) from a sockaddr_in. IPv4 only.
fn sockaddr_v4(sa: &[u8; 32]) -> Option<(u32, u32)> {
    if u16::from_le_bytes([sa[0], sa[1]]) != AF_INET as u16 {
        return None;
    }
    let port = u32::from(u16::from_le_bytes([sa[2], sa[3]]));
    let addr = u32::from_le_bytes([sa[4], sa[5], sa[6], sa[7]]);
    Some((addr, port))
}

// --- real ws2_32 / iphlpapi functions -----------------------------------------------------

static REAL_CONNECT: AtomicUsize = AtomicUsize::new(0);
static REAL_LISTEN: AtomicUsize = AtomicUsize::new(0);
static REAL_GETSOCKNAME: AtomicUsize = AtomicUsize::new(0);
static REAL_TCP_TABLE: AtomicUsize = AtomicUsize::new(0);

type Connect = unsafe extern "system" fn(Socket, *const u8, i32) -> i32;
type Listen = unsafe extern "system" fn(Socket, i32) -> i32;
type GetSockName = unsafe extern "system" fn(Socket, *mut u8, *mut i32) -> i32;
type GetTcpTable = unsafe extern "system" fn(*mut u8, *mut u32, i32, u32, u32, u32) -> u32;

fn local_of(s: Socket) -> Option<(u32, u32)> {
    let mut sa = [0u8; 32];
    let mut len = sa.len() as i32;
    // SAFETY: the real getsockname was stored at install time.
    let real: GetSockName = unsafe { std::mem::transmute(REAL_GETSOCKNAME.load(Ordering::Acquire)) };
    let err = unsafe { WSAGetLastError() };
    let r = unsafe { real(s, sa.as_mut_ptr(), &mut len) };
    unsafe { WSASetLastError(err) };
    if r == 0 { sockaddr_v4(&sa) } else { None }
}

unsafe extern "system" fn hooked_connect(s: Socket, name: *const u8, len: i32) -> i32 {
    // SAFETY: the real connect was stored at install time.
    let real: Connect = unsafe { std::mem::transmute(REAL_CONNECT.load(Ordering::Acquire)) };
    let r = unsafe { real(s, name, len) };
    let err = unsafe { WSAGetLastError() };
    // A non-blocking connect returns WSAEWOULDBLOCK (10035): the connection is already in progress.
    if (r == 0 || err == 10035) && !name.is_null() && len >= 16 {
        let mut sa = [0u8; 32];
        // SAFETY: name points to at least 16 bytes of sockaddr_in.
        unsafe { std::ptr::copy_nonoverlapping(name, sa.as_mut_ptr(), 16) };
        if let (Some(remote), Some(local)) = (sockaddr_v4(&sa), local_of(s)) {
            forget(s);
            record(s, STATE_ESTAB, local, remote);
        }
    }
    unsafe { WSASetLastError(err) };
    r
}

unsafe extern "system" fn hooked_listen(s: Socket, backlog: i32) -> i32 {
    // SAFETY: the real listen was stored at install time.
    let real: Listen = unsafe { std::mem::transmute(REAL_LISTEN.load(Ordering::Acquire)) };
    let r = unsafe { real(s, backlog) };
    if r == 0 {
        let err = unsafe { WSAGetLastError() };
        if let Some(local) = local_of(s) {
            forget(s);
            record(s, STATE_LISTEN, local, (0, 0));
        }
        unsafe { WSASetLastError(err) };
    }
    r
}

/// Called from the closesocket wrapper in winsock.rs.
pub fn on_close(s: Socket) {
    forget(s);
}

/// GetExtendedTcpTable: Wine's answer + rows from the registry (IPv4, classes with owner PIDs).
unsafe extern "system" fn hooked_tcp_table(buf: *mut u8, size: *mut u32, order: i32, af: u32, class: u32, reserved: u32) -> u32 {
    // SAFETY: the real function was stored at install time.
    let real: GetTcpTable = unsafe { std::mem::transmute(REAL_TCP_TABLE.load(Ordering::Acquire)) };
    // Classes 3/4/5: OWNER_PID_LISTENER / CONNECTIONS / ALL. The rest is left alone.
    if af != AF_INET || !(3..=5).contains(&class) || size.is_null() {
        return unsafe { real(buf, size, order, af, class, reserved) };
    }
    let extra: Vec<[u32; 6]> = table()
        .map(|t| {
            t.entries
                .iter()
                .filter(|e| e.used.load(Ordering::Acquire) == 1)
                .filter(|e| match class {
                    3 => e.state == STATE_LISTEN,
                    4 => e.state != STATE_LISTEN,
                    _ => true,
                })
                .filter(|e| alive(e.pid))
                .map(|e| [e.state, e.local_addr, e.local_port, e.remote_addr, e.remote_port, e.pid])
                .collect()
        })
        .unwrap_or_default();

    // First find out how many rows Wine has.
    let mut wine_size = 0u32;
    unsafe { real(std::ptr::null_mut(), &mut wine_size, order, af, class, reserved) };
    let mut wine = vec![0u8; (wine_size as usize).max(4)];
    let mut ws = wine.len() as u32;
    let wr = unsafe { real(wine.as_mut_ptr(), &mut ws, order, af, class, reserved) };
    let wine_rows = if wr == NO_ERROR { u32::from_le_bytes([wine[0], wine[1], wine[2], wine[3]]) as usize } else { 0 };

    let total = wine_rows + extra.len();
    let need = (4 + total * 24) as u32;
    // SAFETY: size was checked for null.
    let have = unsafe { *size };
    unsafe { *size = need };
    if buf.is_null() || have < need {
        return ERROR_INSUFFICIENT_BUFFER;
    }
    // SAFETY: the caller's buffer is at least need bytes.
    unsafe {
        std::ptr::write_unaligned(buf.cast::<u32>(), total as u32);
        if wine_rows > 0 {
            std::ptr::copy_nonoverlapping(wine.as_ptr().add(4), buf.add(4), wine_rows * 24);
        }
        for (i, row) in extra.iter().enumerate() {
            let dst = buf.add(4 + (wine_rows + i) * 24).cast::<u32>();
            for (k, v) in row.iter().enumerate() {
                std::ptr::write_unaligned(dst.add(k), *v);
            }
        }
    }
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < 3 {
        debug(&format!("GetExtendedTcpTable(class {class}): Wine {wine_rows} rows + registry {}", extra.len()));
    }
    NO_ERROR
}

pub fn install() {
    let _ = table();
    // SAFETY: Wine system DLLs; names are null-terminated.
    let ws2 = unsafe { LoadLibraryA(c"ws2_32.dll".as_ptr().cast()) };
    let ip = unsafe { LoadLibraryA(c"iphlpapi.dll".as_ptr().cast()) };
    if ws2.is_null() || ip.is_null() {
        return;
    }
    let get = |m: *mut c_void, n: &std::ffi::CStr| unsafe { GetProcAddress(m, n.as_ptr().cast()) } as usize;
    let (con, lis, gsn, tcp) = (get(ws2, c"connect"), get(ws2, c"listen"), get(ws2, c"getsockname"), get(ip, c"GetExtendedTcpTable"));
    if con == 0 || lis == 0 || gsn == 0 || tcp == 0 {
        return;
    }
    REAL_CONNECT.store(con, Ordering::Release);
    REAL_LISTEN.store(lis, Ordering::Release);
    REAL_GETSOCKNAME.store(gsn, Ordering::Release);
    REAL_TCP_TABLE.store(tcp, Ordering::Release);

    // ws2_32 ordinals: connect = 4, listen = 13. Imports also come by name, so both kinds are patched.
    for (import, hook, label) in [
        (Import::Ordinal(4), hooked_connect as *const () as usize, "tcp registry connect"),
        (Import::Name(b"connect"), hooked_connect as *const () as usize, "tcp registry connect"),
        (Import::Ordinal(13), hooked_listen as *const () as usize, "tcp registry listen"),
        (Import::Name(b"listen"), hooked_listen as *const () as usize, "tcp registry listen"),
    ] {
        iat::register(Patch { target: "*", from: Some("ws2_32.dll"), import, hook, label });
    }
    iat::register(Patch {
        target: "*",
        from: Some("iphlpapi.dll"),
        import: Import::Name(b"GetExtendedTcpTable"),
        hook: hooked_tcp_table as *const () as usize,
        label: "tcp registry GetExtendedTcpTable",
    });
}
