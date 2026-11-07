use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use grace_proto::{Request, Response, read_message, write_message};

/// Порт статистики shim'а dxgi внутри игры (см. shim/dxgi-proxy).
const SHIM_ADDR: &str = "127.0.0.1:47321";

#[derive(Parser)]
#[command(about = "Управление запущенным Elder Grace и shim'ом внутри игры")]
struct Cli {
    /// Путь к Unix-сокету рантайма (по умолчанию тот же, что у Grace.app)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Подключиться по TCP (например, к shim'у внутри Wine)
    #[arg(long, global = true)]
    tcp: Option<String>,
    /// Короткая форма --tcp 127.0.0.1:47321 — shim dxgi внутри игры под эталоном
    #[arg(long, global = true)]
    shim: bool,
    /// Печатать сырой JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Проверить, что рантайм / shim жив
    Ping,
    /// Статистика кадра
    Stats,
    /// Печатать статистику раз в секунду (Ctrl+C — выход)
    Watch,
}

trait Conn: BufRead + Send {
    fn send(&mut self, req: &Request) -> std::io::Result<()>;
}

struct Unix(BufReader<UnixStream>);
struct Tcp(BufReader<TcpStream>);

impl std::io::Read for Unix {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl BufRead for Unix {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.0.fill_buf()
    }
    fn consume(&mut self, n: usize) {
        self.0.consume(n)
    }
}
impl Conn for Unix {
    fn send(&mut self, req: &Request) -> std::io::Result<()> {
        let mut w = self.0.get_ref();
        write_message(&mut w, req)?;
        w.flush()
    }
}
impl std::io::Read for Tcp {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl BufRead for Tcp {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.0.fill_buf()
    }
    fn consume(&mut self, n: usize) {
        self.0.consume(n)
    }
}
impl Conn for Tcp {
    fn send(&mut self, req: &Request) -> std::io::Result<()> {
        let mut w = self.0.get_ref();
        write_message(&mut w, req)?;
        w.flush()
    }
}

fn connect(cli: &Cli) -> Result<Box<dyn Conn>> {
    let tcp = cli.tcp.clone().or_else(|| cli.shim.then(|| SHIM_ADDR.to_string()));
    if let Some(addr) = tcp {
        let s = TcpStream::connect(&addr).with_context(|| format!("нет соединения с {addr} — игра с shim запущена?"))?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        return Ok(Box::new(Tcp(BufReader::new(s))));
    }
    let socket = cli.socket.clone().unwrap_or_else(grace_core::paths::control_socket);
    let s = UnixStream::connect(&socket).with_context(|| format!("нет соединения с {} — Grace.app запущен?", socket.display()))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    Ok(Box::new(Unix(BufReader::new(s))))
}

fn ask(conn: &mut Box<dyn Conn>, req: &Request) -> Result<Response> {
    conn.send(req)?;
    read_message(conn).map_err(anyhow::Error::from)?.context("собеседник закрыл соединение без ответа")
}

fn print(cli: &Cli, response: &Response) -> Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string(response)?);
        return Ok(());
    }
    match response {
        Response::Pong { protocol, app, version } => println!("pong: {app} {version} (протокол {protocol})"),
        Response::Stats(s) => {
            let size = if s.drawable_width > 0 { format!(" · {}×{}", s.drawable_width, s.drawable_height) } else { String::new() };
            println!("кадров {} · {:.1} FPS · {:.2} мс/кадр{size} · {}", s.frames, s.fps, s.frame_ms, s.device);
        }
        Response::Error { message } => bail!("ошибка: {message}"),
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut conn = connect(&cli)?;
    match cli.command {
        Command::Ping => print(&cli, &ask(&mut conn, &Request::Ping)?),
        Command::Stats => print(&cli, &ask(&mut conn, &Request::Stats)?),
        Command::Watch => loop {
            print(&cli, &ask(&mut conn, &Request::Stats)?)?;
            std::thread::sleep(Duration::from_secs(1));
        },
    }
}
