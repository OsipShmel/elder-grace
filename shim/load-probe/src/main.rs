//! Windows probe: loads a DLL from the path argument and calls `grace_shim_version`.
//! Run: `wine64 load-probe.exe graphics_proxy.dll`. Exit code 0 means the DLL is alive.

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::ffi::{c_char, c_void};
    use std::os::windows::ffi::OsStrExt;

    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    if std::env::args().nth(1).as_deref() == Some("bcrypt-rng") {
        return bcrypt_rng_probe();
    }
    if std::env::args().nth(1).as_deref() == Some("tcp-owners") {
        return tcp_owners_probe();
    }
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: load-probe.exe <path to dll>");
        return std::process::ExitCode::from(2);
    };
    let wide: Vec<u16> = path.encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: the string is null-terminated and lives until the end of the call.
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        eprintln!("LoadLibraryW could not load {}", path.to_string_lossy());
        return std::process::ExitCode::from(1);
    }
    // SAFETY: the module is loaded, the name is null-terminated.
    let sym = unsafe { GetProcAddress(module, c"grace_shim_version".as_ptr()) };
    if sym.is_null() {
        eprintln!("no grace_shim_version export");
        return std::process::ExitCode::from(1);
    }
    // SAFETY: the signature matches `extern "C" fn() -> u32` in graphics-proxy.
    let version = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn() -> u32>(sym)() };
    println!("shim loaded, protocol {version}");
    std::process::ExitCode::SUCCESS
}

/// `load-probe.exe bcrypt-rng`: calls BCryptGenRandom with the pseudo-handle
/// BCRYPT_RNG_ALG_HANDLE (0x81), as the Steam client does.
#[cfg(windows)]
fn bcrypt_rng_probe() -> std::process::ExitCode {
    use std::ffi::{c_char, c_void};
    unsafe extern "system" {
        fn LoadLibraryA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    // SAFETY: strings are null-terminated; the BCryptGenRandom signature is from bcrypt.h.
    unsafe {
        let m = LoadLibraryA(c"bcrypt.dll".as_ptr());
        if m.is_null() {
            eprintln!("bcrypt.dll did not load");
            return std::process::ExitCode::from(1);
        }
        let f = GetProcAddress(m, c"BCryptGenRandom".as_ptr());
        let gen_random: extern "system" fn(usize, *mut u8, u32, u32) -> i32 = std::mem::transmute(f);
        let mut buf = [0u8; 16];
        let status = gen_random(0x81, buf.as_mut_ptr(), buf.len() as u32, 0);
        println!("BCryptGenRandom(0x81) -> status {status:#x}, bytes {buf:02x?}");
        if status == 0 && buf.iter().any(|b| *b != 0) {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::from(1)
        }
    }
}

/// `load-probe.exe tcp-owners`: connects to itself over loopback and checks
/// which owner PID GetExtendedTcpTable(TCP_TABLE_OWNER_PID_ALL) reports.
#[cfg(windows)]
fn tcp_owners_probe() -> std::process::ExitCode {
    use std::net::{TcpListener, TcpStream};
    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetExtendedTcpTable(table: *mut u8, size: *mut u32, order: i32, af: u32, class: u32, reserved: u32) -> u32;
    }
    unsafe extern "system" {
        fn GetCurrentProcessId() -> u32;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let client = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let (_server, _) = listener.accept().expect("accept");
    let client_port = client.local_addr().expect("addr").port();
    // SAFETY: GetCurrentProcessId takes no arguments.
    let me = unsafe { GetCurrentProcessId() };
    println!("my PID {me}, listening on {port}, client port {client_port}");
    let mut size = 0u32;
    // SAFETY: the first call gets the size, the second fills a buffer of that size.
    unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, 2, 5, 0) };
    let mut buf = vec![0u8; size as usize + 1024];
    let mut size2 = buf.len() as u32;
    let r = unsafe { GetExtendedTcpTable(buf.as_mut_ptr(), &mut size2, 0, 2, 5, 0) };
    if r != 0 {
        println!("GetExtendedTcpTable returned {r}");
        return std::process::ExitCode::from(1);
    }
    // MIB_TCPTABLE_OWNER_PID: dwNumEntries, then 24-byte rows:
    // state, localAddr, localPort (network order in the low 16 bits), remoteAddr, remotePort, owningPid.
    let n = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4])) as usize;
    println!("rows in the table: {n}");
    let mut ok = false;
    for i in 0..n {
        let row = &buf[4 + i * 24..4 + (i + 1) * 24];
        let f = |o: usize| u32::from_le_bytes(row[o..o + 4].try_into().unwrap_or([0; 4]));
        let lport = u16::from_be(f(8) as u16);
        let rport = u16::from_be(f(16) as u16);
        let pid = f(20);
        if i < 12 || lport == port || lport == client_port || rport == port {
            println!("row: state {} local :{lport} remote :{rport} owningPid {pid}", f(0));
            ok |= pid == me;
        }
    }
    println!("{}", if ok { "owner PID is correct" } else { "owner PID does NOT match" });
    if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::from(1) }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("load-probe builds only for Windows: cargo xtask shim");
}
