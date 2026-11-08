//! Время кадра: отметка на каждом `Present`, CSV-журнал и живая статистика по TCP.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use grace_proto::{PROTOCOL_VERSION, Request, Response, Stats, read_message, write_message};

use crate::debug;

/// DXGI_PRESENT_TEST: проверка без вывода кадра — не считаем.
const DXGI_PRESENT_TEST: u32 = 0x1;
const DEFAULT_PORT: u16 = 47321;

struct State {
    origin: Option<Instant>,
    last: Option<Instant>,
    frames: u64,
    log: Option<BufWriter<File>>,
    window_start: Option<Instant>,
    window_frames: u32,
    window_ms: f64,
    fps: f64,
    frame_ms: f64,
}

static STATE: Mutex<State> = Mutex::new(State {
    origin: None,
    last: None,
    frames: 0,
    log: None,
    window_start: None,
    window_frames: 0,
    window_ms: 0.0,
    fps: 0.0,
    frame_ms: 0.0,
});

/// Открывает журнал и поднимает TCP-сервер статистики (один раз).
pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        if let Some(path) = std::env::var_os("GRACE_FRAMETIME_LOG") {
            match File::create(&path) {
                Ok(f) => {
                    let mut w = BufWriter::new(f);
                    let _ = writeln!(w, "frame,t_ms,dt_ms");
                    if let Ok(mut s) = STATE.lock() {
                        s.log = Some(w);
                    }
                    debug(&format!("журнал времени кадра: {}", path.to_string_lossy()));
                }
                Err(e) => debug(&format!("не открыть журнал кадров: {e}")),
            }
        }
        let port = std::env::var("GRACE_SHIM_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                debug(&format!("статистика: tcp 127.0.0.1:{port}"));
                let _ = std::thread::Builder::new().name("grace-shim-stats".into()).spawn(move || {
                    for conn in listener.incoming().flatten() {
                        let _ = std::thread::Builder::new().name("grace-shim-conn".into()).spawn(move || serve(conn));
                    }
                });
            }
            Err(e) => debug(&format!("порт статистики {port} занят: {e}")),
        }
    });
}

pub fn on_present(flags: u32) {
    if flags & DXGI_PRESENT_TEST != 0 {
        return;
    }
    let now = Instant::now();
    let Ok(mut s) = STATE.lock() else { return };
    let origin = *s.origin.get_or_insert(now);
    let dt = s.last.map_or(0.0, |l| (now - l).as_secs_f64() * 1000.0);
    s.last = Some(now);
    s.frames += 1;
    let frame = s.frames;
    if let Some(w) = s.log.as_mut() {
        let _ = writeln!(w, "{frame},{:.3},{dt:.3}", (now - origin).as_secs_f64() * 1000.0);
        if frame.is_multiple_of(120) {
            let _ = w.flush();
        }
    }
    // Окно в 1 секунду для живой статистики.
    let window_start = *s.window_start.get_or_insert(now);
    if dt > 0.0 {
        s.window_frames += 1;
        s.window_ms += dt;
    }
    let elapsed = (now - window_start).as_secs_f64();
    if elapsed >= 1.0 && s.window_frames > 0 {
        s.fps = f64::from(s.window_frames) / elapsed;
        s.frame_ms = s.window_ms / f64::from(s.window_frames);
        s.window_start = Some(now);
        s.window_frames = 0;
        s.window_ms = 0.0;
    }
}

fn snapshot() -> Stats {
    let s = STATE.lock().ok();
    Stats {
        frames: s.as_ref().map_or(0, |s| s.frames),
        frame_ms: s.as_ref().map_or(0.0, |s| s.frame_ms),
        fps: s.as_ref().map_or(0.0, |s| s.fps),
        drawable_width: 0,
        drawable_height: 0,
        device: "игра под эталоном (shim dxgi)".into(),
    }
}

fn serve(conn: TcpStream) {
    let Ok(mut writer) = conn.try_clone() else { return };
    let mut reader = BufReader::new(conn);
    loop {
        let response = match read_message::<_, Request>(&mut reader) {
            Ok(None) => return,
            Ok(Some(Request::Ping)) => Response::Pong {
                protocol: PROTOCOL_VERSION,
                app: "shim-dxgi".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            Ok(Some(Request::Stats)) => Response::Stats(snapshot()),
            Ok(Some(Request::Overrides { .. } | Request::Bound { .. } | Request::Camera { .. })) => Response::Error { message: "подмены — в shim d3d12 (порт 47322)".into() },
            Err(e) => Response::Error { message: format!("неверный запрос: {e}") },
        };
        if write_message(&mut writer, &response).is_err() {
            return;
        }
    }
}
