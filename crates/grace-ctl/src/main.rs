use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use grace_proto::{CameraLayout, OverrideOp, Request, Response, read_message, write_message};

/// Stats port of the dxgi shim inside the game (see shim/dxgi-proxy).
const SHIM_ADDR: &str = "127.0.0.1:47321";
/// Override control port of the d3d12 shim inside the game (see shim/graphics-proxy/src/live.rs).
const SHIM_D3D12_ADDR: &str = "127.0.0.1:47322";

#[derive(Parser)]
#[command(about = "Control a running Elder Grace runtime and the shim inside the game")]
struct Cli {
    /// Path to the runtime Unix socket (defaults to the one Grace.app uses)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Connect over TCP (for example, to a shim inside Wine)
    #[arg(long, global = true)]
    tcp: Option<String>,
    /// Short form of --tcp 127.0.0.1:47321: the dxgi shim inside the game under the oracle
    #[arg(long, global = true)]
    shim: bool,
    /// Print raw JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that the runtime / shim is alive
    Ping,
    /// Frame statistics
    Stats,
    /// Print statistics once a second (Ctrl+C to quit)
    Watch,
    /// Which shaders are drawing right now (d3d12 shim): records PSO bindings over a window
    Bound {
        /// Recording window, ms
        #[arg(long, default_value_t = 1000)]
        ms: u32,
        /// How many rows to show
        #[arg(long, default_value_t = 40)]
        top: usize,
        /// Pixel shaders only
        #[arg(long)]
        ps: bool,
    },
    /// Camera from the live game (d3d12 shim): finds the cbuffer by a layout from the game's JSON
    Camera {
        /// Camera cbuffer layout (for example, ../grace-eldenring/data/camera-layout.json)
        #[arg(long)]
        layout: PathBuf,
        /// Save the first camera found as JSON (for a viewer's --camera)
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// Debug: read game memory (d3d12 shim) and show it as f32
    Peek {
        /// Address (0x…)
        addr: String,
        #[arg(default_value_t = 256)]
        len: u32,
    },
    /// Debug: find consecutive f32 values in game memory within "min:max" ranges (one per value)
    FindFloats {
        ranges: Vec<String>,
        #[arg(long, default_value_t = 64)]
        limit: u32,
    },
    /// Shader overrides in the game (d3d12 shim, 127.0.0.1:47322 by default)
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
    /// Re-read the override directory and recreate overridden PSOs
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
    let d3d12 = matches!(cli.command, Command::Overrides { .. } | Command::Bound { .. } | Command::Camera { .. } | Command::Peek { .. } | Command::FindFloats { .. }) && cli.socket.is_none();
    let tcp = cli
        .tcp
        .clone()
        .or_else(|| (cli.shim && !d3d12).then(|| SHIM_ADDR.to_string()))
        .or_else(|| d3d12.then(|| SHIM_D3D12_ADDR.to_string()));
    if let Some(addr) = tcp {
        let s = TcpStream::connect(&addr).with_context(|| format!("cannot connect to {addr}: is the game running with the shim?"))?;
        s.set_read_timeout(Some(Duration::from_secs(300)))?;
        return Ok(Box::new(Tcp(BufReader::new(s))));
    }
    let socket = cli.socket.clone().unwrap_or_else(grace_core::paths::control_socket);
    let s = UnixStream::connect(&socket).with_context(|| format!("cannot connect to {}: is Grace.app running?", socket.display()))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    Ok(Box::new(Unix(BufReader::new(s))))
}

fn ask(conn: &mut Box<dyn Conn>, req: &Request) -> Result<Response> {
    conn.send(req)?;
    read_message(conn).map_err(anyhow::Error::from)?.context("peer closed the connection without a reply")
}

fn print_bound(cli: &Cli, response: &Response, top: usize, ps_only: bool) -> Result<()> {
    let Response::Bound(r) = response else { return print(cli, response) };
    if cli.json {
        println!("{}", serde_json::to_string(response)?);
        return Ok(());
    }
    println!("PSOs bound in {} ms: {} (without a recipe: {}), shaders: {}", r.ms, r.pipelines, r.unknown, r.shaders.len());
    println!("{:>8} {:>5}  {:<5} shader", "binds", "PSO", "kind");
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
        Response::Pong { protocol, app, version } => println!("pong: {app} {version} (protocol {protocol})"),
        Response::Stats(s) => {
            let size = if s.drawable_width > 0 { format!(" · {}×{}", s.drawable_width, s.drawable_height) } else { String::new() };
            println!("{} frames · {:.1} FPS · {:.2} ms/frame{size} · {}", s.frames, s.fps, s.frame_ms, s.device);
        }
        Response::Overrides(o) => {
            println!(
                "overrides {} · files {} · PSO pairs {} (failed {}) · substitutions {} · recipes {}",
                if o.enabled { "ON" } else { "off" },
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
        Response::Bound(r) => println!("shaders in {} ms: {}", r.ms, r.shaders.len()),
        Response::Camera(r) => {
            println!("memory regions {} ({} MB), camera candidates {}", r.buffers, r.bytes >> 20, r.candidates.len());
            for c in &r.candidates {
                // column_major: [r][c] -> index c*4 + r; forward is column 2, P[1][1] = cot(fovy/2).
                let fwd = [c.inv_view[8], c.inv_view[9], c.inv_view[10]];
                let fovy = 2.0 * (1.0 / c.proj[5]).atan();
                let screen = c.screen.map(|s| format!(" · screen {}x{}", s[0], s[1])).unwrap_or_default();
                println!(
                    "  position ({:.2}, {:.2}, {:.2}) · forward ({:.3}, {:.3}, {:.3}) · fovy {:.1}°{screen}",
                    c.position[0], c.position[1], c.position[2], fwd[0], fwd[1], fwd[2], fovy.to_degrees()
                );
            }
            if !r.same_rotation.is_empty() {
                println!("matrices with the same rotation and a different translation: {}", r.same_rotation.len());
                for (addr, t) in &r.same_rotation {
                    println!("  {addr:#x} · translation ({:.2}, {:.2}, {:.2})", t[0], t[1], t[2]);
                }
            }
            if !r.projections.is_empty() {
                println!("perspective projections in memory: {} (first ones)", r.projections.len());
                for (addr, fovy, aspect) in r.projections.iter().take(12) {
                    println!("  {addr:#x} · fovy {fovy:.1}° · P00/P11 {aspect:.3}");
                }
            }
        }
        Response::Peek { addr, bytes } => {
            println!("{addr:#x}: {} bytes", bytes.len());
            for (i, row) in bytes.as_chunks::<16>().0.iter().enumerate() {
                let f: Vec<String> = row.as_chunks::<4>().0.iter().map(|b| format!("{:>12.4}", f32::from_le_bytes(*b))).collect();
                println!("  +{:#06x} {}", i * 16, f.join(" "));
            }
        }
        Response::Found { regions, addrs } => {
            println!("regions {regions}, found {}", addrs.len());
            for a in addrs {
                println!("  {a:#x}");
            }
        }
        Response::Modules { modules } => {
            for m in modules {
                println!("{:#014x} {:>10} {}", m.base, m.size, m.name);
            }
        }
        Response::Mirror { generation } => println!("mirror: region set {generation}"),
        Response::Error { message } => bail!("error: {message}"),
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
            let layout: CameraLayout = serde_json::from_str(&std::fs::read_to_string(layout).with_context(|| format!("reading {}", layout.display()))?)?;
            let r = ask(&mut conn, &Request::Camera { layout })?;
            print(&cli, &r)?;
            if let (Some(out), Response::Camera(rep)) = (save, &r) {
                let mut first = rep.candidates.first().context("camera not found, nothing to save")?.clone();
                first.world = rep.world();
                if let Some(w) = first.world {
                    println!("world position: ({:.2}, {:.2}, {:.2}), local origin ({:.2}, {:.2}, {:.2})", w[0], w[1], w[2], w[0] - first.position[0], w[1] - first.position[1], w[2] - first.position[2]);
                }
                std::fs::write(out, serde_json::to_string_pretty(&first)?)?;
                println!("saved: {}", out.display());
            }
            Ok(())
        }
        Command::Peek { ref addr, len } => {
            let a = u64::from_str_radix(addr.trim_start_matches("0x"), 16).with_context(|| format!("address {addr}"))?;
            print(&cli, &ask(&mut conn, &Request::Peek { addr: a, len })?)
        }
        Command::FindFloats { ref ranges, limit } => {
            let ranges = ranges
                .iter()
                .map(|r| {
                    let (lo, hi) = r.split_once(':').with_context(|| format!("range \"{r}\": expected min:max"))?;
                    Ok((lo.parse::<f32>()?, hi.parse::<f32>()?))
                })
                .collect::<Result<Vec<_>>>()?;
            print(&cli, &ask(&mut conn, &Request::FindFloats { ranges, limit })?)
        }
        Command::Overrides { op } => print(&cli, &ask(&mut conn, &Request::Overrides { op: op.into() })?),
        Command::Watch => loop {
            print(&cli, &ask(&mut conn, &Request::Stats)?)?;
            std::thread::sleep(Duration::from_secs(1));
        },
    }
}
