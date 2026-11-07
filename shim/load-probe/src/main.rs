//! Windows-пробник: грузит DLL по пути из аргумента и вызывает `grace_shim_version`.
//! Запуск: `wine64 load-probe.exe graphics_proxy.dll`. Код выхода 0 — DLL жива.

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
        eprintln!("использование: load-probe.exe <путь к dll>");
        return std::process::ExitCode::from(2);
    };
    let wide: Vec<u16> = path.encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: строка нуль-терминирована и живёт до конца вызова.
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        eprintln!("LoadLibraryW не смог загрузить {}", path.to_string_lossy());
        return std::process::ExitCode::from(1);
    }
    // SAFETY: модуль загружен, имя нуль-терминировано.
    let sym = unsafe { GetProcAddress(module, c"grace_shim_version".as_ptr()) };
    if sym.is_null() {
        eprintln!("нет экспорта grace_shim_version");
        return std::process::ExitCode::from(1);
    }
    // SAFETY: сигнатура совпадает с `extern "C" fn() -> u32` в graphics-proxy.
    let version = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn() -> u32>(sym)() };
    println!("shim загружен, протокол {version}");
    std::process::ExitCode::SUCCESS
}

/// `load-probe.exe bcrypt-rng`: вызывает BCryptGenRandom с псевдо-хендлом
/// BCRYPT_RNG_ALG_HANDLE (0x81), как это делает клиент Steam.
#[cfg(windows)]
fn bcrypt_rng_probe() -> std::process::ExitCode {
    use std::ffi::{c_char, c_void};
    unsafe extern "system" {
        fn LoadLibraryA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    // SAFETY: строки нуль-терминированы; сигнатура BCryptGenRandom из bcrypt.h.
    unsafe {
        let m = LoadLibraryA(c"bcrypt.dll".as_ptr());
        if m.is_null() {
            eprintln!("bcrypt.dll не загрузился");
            return std::process::ExitCode::from(1);
        }
        let f = GetProcAddress(m, c"BCryptGenRandom".as_ptr());
        let gen_random: extern "system" fn(usize, *mut u8, u32, u32) -> i32 = std::mem::transmute(f);
        let mut buf = [0u8; 16];
        let status = gen_random(0x81, buf.as_mut_ptr(), buf.len() as u32, 0);
        println!("BCryptGenRandom(0x81) → status {status:#x}, байты {buf:02x?}");
        if status == 0 && buf.iter().any(|b| *b != 0) {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::from(1)
        }
    }
}

/// `load-probe.exe tcp-owners`: соединяется сам с собой по loopback и смотрит,
/// какой PID владельца отдаёт GetExtendedTcpTable(TCP_TABLE_OWNER_PID_ALL).
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
    // SAFETY: GetCurrentProcessId без аргументов.
    let me = unsafe { GetCurrentProcessId() };
    println!("мой PID {me}, слушаю {port}, клиентский порт {client_port}");
    let mut size = 0u32;
    // SAFETY: первый вызов узнаёт размер, второй заполняет буфер этого размера.
    unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, 2, 5, 0) };
    let mut buf = vec![0u8; size as usize + 1024];
    let mut size2 = buf.len() as u32;
    let r = unsafe { GetExtendedTcpTable(buf.as_mut_ptr(), &mut size2, 0, 2, 5, 0) };
    if r != 0 {
        println!("GetExtendedTcpTable вернул {r}");
        return std::process::ExitCode::from(1);
    }
    // MIB_TCPTABLE_OWNER_PID: dwNumEntries, затем строки по 24 байта:
    // state, localAddr, localPort(net order в младших 16 битах), remoteAddr, remotePort, owningPid.
    let n = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4])) as usize;
    println!("строк в таблице: {n}");
    let mut ok = false;
    for i in 0..n {
        let row = &buf[4 + i * 24..4 + (i + 1) * 24];
        let f = |o: usize| u32::from_le_bytes(row[o..o + 4].try_into().unwrap_or([0; 4]));
        let lport = u16::from_be(f(8) as u16);
        let rport = u16::from_be(f(16) as u16);
        let pid = f(20);
        if i < 12 || lport == port || lport == client_port || rport == port {
            println!("строка: state {} local :{lport} remote :{rport} owningPid {pid}", f(0));
            ok |= pid == me;
        }
    }
    println!("{}", if ok { "PID владельца верный" } else { "PID владельца НЕ совпадает" });
    if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::from(1) }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("load-probe собирается только под Windows: cargo xtask shim");
}
