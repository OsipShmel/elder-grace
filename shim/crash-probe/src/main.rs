//! `crash-probe.exe` — замена `winedbg --auto` для префиксов Elder Grace.
//!
//! Wine при необработанном исключении запускает отладчик из
//! `HKLM\Software\Microsoft\Windows NT\CurrentVersion\AeDebug\Debugger`
//! с аргументами `<pid> <event>`. В GPTK `winedbg` нет, поэтому падения немые.
//! Этот отладчик подключается к процессу, ловит исключение и пишет отчёт:
//! код, адрес как `модуль+оффсет`, регистры, кандидаты в адреса возврата со стека
//! (значения, указывающие внутрь модулей сразу после инструкции `call`), список модулей.
//!
//! Отчёты: `C:\grace-crash\<pid>-<tid>.txt` (в префиксе) и stdout.

#[cfg(all(windows, target_arch = "x86_64"))]
mod imp {
    use std::ffi::c_void;
    use std::fmt::Write as _;

    type Handle = *mut c_void;

    #[repr(C)]
    struct DebugEvent {
        code: u32,
        pid: u32,
        tid: u32,
        _pad: u32,
        // union u: EXCEPTION_DEBUG_INFO и др.; первые поля — EXCEPTION_RECORD.
        u: [u64; 20],
    }

    /// CONTEXT x64: нужны только целочисленные регистры и RIP (смещение 0x78..0x100).
    #[repr(C, align(16))]
    struct Context {
        home: [u64; 6],
        flags: u32,
        mxcsr: u32,
        seg: [u16; 6],
        eflags: u32,
        dr: [u64; 6],
        regs: [u64; 16], // rax rcx rdx rbx rsp rbp rsi rdi r8..r15
        rip: u64,
        rest: [u8; 976],
    }

    #[repr(C)]
    struct ModuleInfo {
        base: *mut c_void,
        size: u32,
        entry: *mut c_void,
    }

    const EXCEPTION_DEBUG_EVENT: u32 = 1;
    const EXIT_PROCESS_DEBUG_EVENT: u32 = 5;
    const DBG_CONTINUE: u32 = 0x0001_0002;
    const DBG_EXCEPTION_NOT_HANDLED: u32 = 0x8001_0001;
    const CONTEXT_CONTROL_INTEGER: u32 = 0x0010_0003;
    const THREAD_ALL_ACCESS: u32 = 0x001F_FFFF;
    const PROCESS_ALL_ACCESS: u32 = 0x001F_FFFF;
    const INFINITE: u32 = 0xFFFF_FFFF;

    unsafe extern "system" {
        fn DebugActiveProcess(pid: u32) -> i32;
        fn WaitForDebugEvent(ev: *mut DebugEvent, ms: u32) -> i32;
        fn ContinueDebugEvent(pid: u32, tid: u32, status: u32) -> i32;
        fn SetEvent(h: Handle) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn OpenThread(access: u32, inherit: i32, tid: u32) -> Handle;
        fn GetThreadContext(h: Handle, ctx: *mut Context) -> i32;
        fn ReadProcessMemory(h: Handle, addr: *const c_void, buf: *mut c_void, len: usize, read: *mut usize) -> i32;
        fn K32EnumProcessModulesEx(h: Handle, mods: *mut Handle, cb: u32, needed: *mut u32, filter: u32) -> i32;
        fn K32GetModuleBaseNameW(h: Handle, m: Handle, name: *mut u16, size: u32) -> u32;
        fn K32GetModuleInformation(h: Handle, m: Handle, info: *mut ModuleInfo, cb: u32) -> i32;
        fn TerminateProcess(h: Handle, code: u32) -> i32;
        fn CloseHandle(h: Handle) -> i32;
    }

    struct Module {
        name: String,
        base: u64,
        size: u64,
    }

    fn modules(process: Handle) -> Vec<Module> {
        let mut handles = vec![std::ptr::null_mut::<c_void>(); 1024];
        let mut needed = 0u32;
        // SAFETY: буфер на 1024 хендла, размер передан в байтах.
        let ok = unsafe {
            K32EnumProcessModulesEx(process, handles.as_mut_ptr(), (handles.len() * 8) as u32, &mut needed, 3)
        };
        if ok == 0 {
            return Vec::new();
        }
        handles.truncate((needed as usize / 8).min(handles.len()));
        let mut out = Vec::new();
        for m in handles {
            let mut name = [0u16; 260];
            let mut info = ModuleInfo { base: std::ptr::null_mut(), size: 0, entry: std::ptr::null_mut() };
            // SAFETY: буферы корректного размера, хендлы получены выше.
            let n = unsafe { K32GetModuleBaseNameW(process, m, name.as_mut_ptr(), 260) } as usize;
            if unsafe { K32GetModuleInformation(process, m, &mut info, size_of::<ModuleInfo>() as u32) } == 0 {
                continue;
            }
            out.push(Module {
                name: String::from_utf16_lossy(&name[..n]),
                base: info.base as u64,
                size: u64::from(info.size),
            });
        }
        out.sort_by_key(|m| m.base);
        out
    }

    fn locate(mods: &[Module], addr: u64) -> String {
        mods.iter()
            .find(|m| addr >= m.base && addr < m.base + m.size)
            .map_or_else(|| format!("{addr:#x} (вне модулей)"), |m| format!("{}+{:#x}", m.name, addr - m.base))
    }

    fn read(process: Handle, addr: u64, buf: &mut [u8]) -> usize {
        let mut got = 0usize;
        // SAFETY: буфер наш, чтение чужой памяти безопасно для нас при любом исходе.
        unsafe { ReadProcessMemory(process, addr as *const c_void, buf.as_mut_ptr().cast(), buf.len(), &mut got) };
        got
    }

    /// Похоже ли, что перед `addr` стоит инструкция call (E8 rel32, FF /2 с разными ModRM).
    fn after_call(process: Handle, addr: u64) -> bool {
        let mut b = [0u8; 7];
        if read(process, addr.saturating_sub(7), &mut b) != 7 {
            return false;
        }
        b[2] == 0xE8 // call rel32 (5 байт)
            || (b[1] == 0xFF && b[2] & 0x38 == 0x10) // call [rip+disp32] (6 байт)
            || (b[5] == 0xFF && b[6] & 0x38 == 0x10) // call reg (2 байта)
            || (b[4] == 0xFF && b[5] & 0x38 == 0x10) // call [reg+disp8] (3 байта)
            || (b[0] == 0xFF && b[1] & 0x38 == 0x10) // call [reg+disp32] (7 байт)
    }

    /// Читаемая ASCII- или UTF-16-строка длиной от 8 символов по адресу.
    fn string_at(process: Handle, addr: u64) -> Option<String> {
        let mut buf = [0u8; 512];
        let got = read(process, addr, &mut buf);
        if got < 16 {
            return None;
        }
        let buf = &buf[..got];
        let printable = |b: u8| (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\t';
        let ascii: Vec<u8> = buf.iter().copied().take_while(|b| *b != 0).collect();
        if ascii.len() >= 8 && ascii.iter().all(|b| printable(*b)) {
            return Some(String::from_utf8_lossy(&ascii).trim_end().to_string());
        }
        let wide: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
        if wide.len() >= 8 && wide.iter().all(|c| *c < 0x7f && printable(*c as u8)) {
            return Some(format!("(utf16) {}", String::from_utf16_lossy(&wide).trim_end()));
        }
        None
    }

    fn report(process: Handle, pid: u32, tid: u32, code: u32, addr: u64) -> String {
        let mods = modules(process);
        let mut s = String::new();
        let _ = writeln!(s, "grace crash-probe: pid {pid:#x} tid {tid:#x}");
        let _ = writeln!(s, "исключение {code:#010x} по адресу {}", locate(&mods, addr));

        // SAFETY: OpenThread/GetThreadContext с корректной структурой CONTEXT (выравнивание 16).
        let thread = unsafe { OpenThread(THREAD_ALL_ACCESS, 0, tid) };
        let mut ctx: Context = unsafe { std::mem::zeroed() };
        ctx.flags = CONTEXT_CONTROL_INTEGER;
        if !thread.is_null() && unsafe { GetThreadContext(thread, &mut ctx) } != 0 {
            let names = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"];
            let _ = writeln!(s, "rip {:#x} = {}", ctx.rip, locate(&mods, ctx.rip));
            for (n, v) in names.iter().zip(ctx.regs) {
                let _ = writeln!(s, "  {n:>3} {v:#018x}  {}", if mods.iter().any(|m| v >= m.base && v < m.base + m.size) { locate(&mods, v) } else { String::new() });
            }
            let rsp = ctx.regs[4];
            let mut stack = vec![0u8; 0x8000];
            let got = read(process, rsp, &mut stack);

            // Строки, на которые указывают регистры и значения со стека: так видно
            // текст фатальной проверки (`Check failed: …`, `file.cc(123)`), даже без символов.
            let _ = writeln!(s, "строки рядом с падением:");
            let mut seen = std::collections::BTreeSet::new();
            let candidates = ctx.regs.iter().copied().chain(
                stack[..got].chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap_or([0; 8]))),
            );
            for p in candidates {
                if p < 0x10000 || !seen.insert(p) {
                    continue;
                }
                if let Some(text) = string_at(process, p) {
                    let _ = writeln!(s, "  {p:#x}: {text}");
                    if seen.len() > 400 {
                        break;
                    }
                }
            }
            let _ = writeln!(s, "адреса возврата (эвристика, сверху стека):");
            let mut shown = 0;
            for (i, chunk) in stack[..got].chunks_exact(8).enumerate() {
                let v = u64::from_le_bytes(chunk.try_into().unwrap_or([0; 8]));
                if mods.iter().any(|m| v >= m.base && v < m.base + m.size) && after_call(process, v) {
                    let _ = writeln!(s, "  [rsp+{:#06x}] {}", i * 8, locate(&mods, v));
                    shown += 1;
                    if shown >= 40 {
                        break;
                    }
                }
            }
        }
        if !thread.is_null() {
            // SAFETY: хендл получен выше.
            unsafe { CloseHandle(thread) };
        }
        let _ = writeln!(s, "модули:");
        for m in &mods {
            let _ = writeln!(s, "  {:#014x} {:#010x} {}", m.base, m.size, m.name);
        }
        s
    }

    pub fn main() -> std::process::ExitCode {
        let args: Vec<String> = std::env::args().collect();
        let (Some(pid), event) = (args.get(1).and_then(|a| a.parse::<u32>().ok()), args.get(2).and_then(|a| a.parse::<usize>().ok())) else {
            eprintln!("использование: crash-probe.exe <pid> [event]");
            return std::process::ExitCode::from(2);
        };
        // SAFETY: обычные вызовы отладочного API с проверкой результата.
        if unsafe { DebugActiveProcess(pid) } == 0 {
            eprintln!("crash-probe: DebugActiveProcess({pid}) не удался");
            return std::process::ExitCode::from(1);
        }
        if let Some(ev) = event {
            // SAFETY: хендл события унаследован от Wine (протокол AeDebug).
            unsafe { SetEvent(ev as Handle) };
        }
        let process = unsafe { OpenProcess(PROCESS_ALL_ACCESS, 0, pid) };
        let mut ev: DebugEvent = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: структура нужного размера.
            if unsafe { WaitForDebugEvent(&mut ev, INFINITE) } == 0 {
                break;
            }
            let mut status = DBG_CONTINUE;
            if ev.code == EXCEPTION_DEBUG_EVENT {
                let code = ev.u[0] as u32;
                let addr = ev.u[2];
                let first_chance = (ev.u[19] & 0xFFFF_FFFF) as u32;
                let mods = modules(process);
                let in_ntdll = locate(&mods, addr).starts_with("ntdll");
                // Первая точка останова при подключении — из ntdll (DbgBreakPoint), её пропускаем.
                if !(code == 0x8000_0003 && in_ntdll) {
                    let text = report(process, pid, ev.tid, code, addr);
                    print!("{text}");
                    let dir = "C:\\grace-crash";
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(format!("{dir}\\{pid}-{}.txt", ev.tid), &text);
                    let _ = first_chance;
                    // SAFETY: процесс всё равно умирает; завершаем, чтобы не висел.
                    unsafe { TerminateProcess(process, code) };
                    status = DBG_EXCEPTION_NOT_HANDLED;
                }
            }
            // SAFETY: продолжаем событие, которое только что получили.
            unsafe { ContinueDebugEvent(ev.pid, ev.tid, status) };
            if ev.code == EXIT_PROCESS_DEBUG_EVENT {
                break;
            }
        }
        std::process::ExitCode::SUCCESS
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn main() -> std::process::ExitCode {
    imp::main()
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    eprintln!("crash-probe собирается только под Windows x86_64: cargo xtask shim");
}
