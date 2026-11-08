//! Сервер управления: Unix-сокет, протокол `grace-proto`.

use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use grace_proto::{PROTOCOL_VERSION, Request, Response, Stats, read_message, write_message};

pub type SharedStats = Arc<Mutex<Stats>>;

pub struct ControlServer {
    path: PathBuf,
}

impl ControlServer {
    pub fn start(path: &Path, stats: SharedStats) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("не создаётся {}", dir.display()))?;
        }
        // Сокет от прошлого запуска мешает bind; живой процесс мы бы не задели,
        // но второй экземпляр приложения заберёт сокет себе — пока это приемлемо.
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).with_context(|| format!("bind {}", path.display()))?;
        eprintln!("[grace] управление: {}", path.display());

        thread::Builder::new().name("grace-control".into()).spawn(move || {
            for conn in listener.incoming().flatten() {
                let stats = Arc::clone(&stats);
                let _ = thread::Builder::new()
                    .name("grace-control-conn".into())
                    .spawn(move || handle(conn, &stats));
            }
        })?;
        Ok(Self { path: path.to_owned() })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn handle(conn: UnixStream, stats: &SharedStats) {
    let Ok(mut writer) = conn.try_clone() else { return };
    let mut reader = BufReader::new(conn);
    loop {
        let response = match read_message::<_, Request>(&mut reader) {
            Ok(None) => return,
            Ok(Some(Request::Ping)) => Response::Pong {
                protocol: PROTOCOL_VERSION,
                app: "grace-app".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            Ok(Some(Request::Stats)) => match stats.lock() {
                Ok(s) => Response::Stats(s.clone()),
                Err(_) => Response::Error { message: "статистика недоступна".into() },
            },
            Ok(Some(Request::Overrides { .. } | Request::Bound { .. })) => Response::Error { message: "подмены шейдеров есть только в shim d3d12".into() },
            Err(e) => Response::Error { message: format!("неверный запрос: {e}") },
        };
        if write_message(&mut writer, &response).is_err() {
            return;
        }
    }
}
