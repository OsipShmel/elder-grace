use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use grace_proto::{CameraLayout, OverrideOp, Request, Response, read_message, write_message};

/// Порт статистики shim'а dxgi внутри игры (см. shim/dxgi-proxy).
const SHIM_ADDR: &str = "127.0.0.1:47321";
/// Порт управления подменами shim'а d3d12 внутри игры (см. shim/graphics-proxy/src/live.rs).
const SHIM_D3D12_ADDR: &str = "127.0.0.1:47322";

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
    /// Какие шейдеры рисуют сейчас (shim d3d12): запись привязок PSO за окно
    Bound {
        /// Окно записи, мс
        #[arg(long, default_value_t = 1000)]
        ms: u32,
        /// Сколько строк показать
        #[arg(long, default_value_t = 40)]
        top: usize,
        /// Только пиксельные шейдеры
        #[arg(long)]
        ps: bool,
    },
    /// Камера из живой игры (shim d3d12): поиск cbuffer'а по раскладке из JSON игры
    Camera {
        /// Раскладка cbuffer'а камеры (например, ../grace-eldenring/data/camera-layout.json)
        #[arg(long)]
        layout: PathBuf,
        /// Сохранить первую найденную камеру в JSON (для er-viewer --camera)
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// Подмены шейдеров в игре (shim d3d12, по умолчанию 127.0.0.1:47322)
    Overrides {
        #[arg(value_enum, default_value_t = Op::Status)]
        op: Op,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Op {
    Status,
    On,
    Off,
    Toggle,
    /// Перечитать каталог подмен и пересоздать подменённые PSO
    Reload,
}

impl From<Op> for OverrideOp {
    fn from(op: Op) -> Self {
        match op {
            Op::Status => OverrideOp::Status,
            Op::On => OverrideOp::On,
            Op::Off => OverrideOp::Off,
            Op::Toggle => OverrideOp::Toggle,
            Op::Reload => OverrideOp::Reload,
        }
    }
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
    let d3d12 = matches!(cli.command, Command::Overrides { .. } | Command::Bound { .. } | Command::Camera { .. }) && cli.socket.is_none();
    let tcp = cli
        .tcp
        .clone()
        .or_else(|| (cli.shim && !d3d12).then(|| SHIM_ADDR.to_string()))
        .or_else(|| d3d12.then(|| SHIM_D3D12_ADDR.to_string()));
    if let Some(addr) = tcp {
        let s = TcpStream::connect(&addr).with_context(|| format!("нет соединения с {addr} — игра с shim запущена?"))?;
        s.set_read_timeout(Some(Duration::from_secs(60)))?;
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

fn print_bound(cli: &Cli, response: &Response, top: usize, ps_only: bool) -> Result<()> {
    let Response::Bound(r) = response else { return print(cli, response) };
    if cli.json {
        println!("{}", serde_json::to_string(response)?);
        return Ok(());
    }
    println!("за {} мс привязано PSO: {} (без рецепта: {}), шейдеров: {}", r.ms, r.pipelines, r.unknown, r.shaders.len());
    println!("{:>8} {:>5}  {:<5} шейдер", "привязок", "PSO", "тип");
    for s in r.shaders.iter().filter(|s| !ps_only || s.name.ends_with(".ps")).take(top) {
        println!("{:>8} {:>5}  {:<5} {}", s.binds, s.pipelines, if s.dxil { "DXIL" } else { "DXBC" }, s.name);
    }
    Ok(())
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
        Response::Overrides(o) => {
            println!(
                "подмены {} · файлов {} · пар PSO {} (не создано {}) · подстановок {} · рецептов {}",
                if o.enabled { "ВКЛ" } else { "выкл" },
                o.shaders_loaded,
                o.pairs,
                o.failed,
                o.swaps,
                o.recipes
            );
            if !o.last_reload.is_empty() {
                println!("{}", o.last_reload);
            }
            for name in &o.applied {
                println!("  {name}");
            }
        }
        Response::Bound(r) => println!("за {} мс шейдеров: {}", r.ms, r.shaders.len()),
        Response::Camera(r) => {
            println!("отображённых буферов {} ({} МБ), кандидатов-камер {}", r.buffers, r.bytes >> 20, r.candidates.len());
            for c in &r.candidates {
                // column_major: [r][c] → индекс c·4 + r; вперёд — столбец 2, P[1][1] = ctg(fovy/2).
                let fwd = [c.inv_view[8], c.inv_view[9], c.inv_view[10]];
                let fovy = 2.0 * (1.0 / c.proj[5]).atan();
                let screen = c.screen.map(|s| format!(" · экран {}×{}", s[0], s[1])).unwrap_or_default();
                println!(
                    "  позиция ({:.2}, {:.2}, {:.2}) · вперёд ({:.3}, {:.3}, {:.3}) · fovy {:.1}°{screen}",
                    c.position[0], c.position[1], c.position[2], fwd[0], fwd[1], fwd[2], fovy.to_degrees()
                );
            }
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
        Command::Bound { ms, top, ps } => {
            let r = ask(&mut conn, &Request::Bound { ms })?;
            print_bound(&cli, &r, top, ps)
        }
        Command::Camera { ref layout, ref save } => {
            let layout: CameraLayout = serde_json::from_str(&std::fs::read_to_string(layout).with_context(|| format!("чтение {}", layout.display()))?)?;
            let r = ask(&mut conn, &Request::Camera { layout })?;
            print(&cli, &r)?;
            if let (Some(out), Response::Camera(rep)) = (save, &r) {
                let first = rep.candidates.first().context("камера не найдена — нечего сохранять")?;
                std::fs::write(out, serde_json::to_string_pretty(first)?)?;
                println!("сохранено: {}", out.display());
            }
            Ok(())
        }
        Command::Overrides { op } => print(&cli, &ask(&mut conn, &Request::Overrides { op: op.into() })?),
        Command::Watch => loop {
            print(&cli, &ask(&mut conn, &Request::Stats)?)?;
            std::thread::sleep(Duration::from_secs(1));
        },
    }
}
