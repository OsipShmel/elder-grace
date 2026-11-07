use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use grace_proto::{Request, Response, read_message, write_message};

#[derive(Parser)]
#[command(about = "Управление запущенным Elder Grace")]
struct Cli {
    /// Путь к сокету (по умолчанию тот же, что у рантайма)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Печатать сырой JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Проверить, что рантайм жив
    Ping,
    /// Статистика кадра
    Stats,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(grace_core::paths::control_socket);
    let request = match cli.command {
        Command::Ping => Request::Ping,
        Command::Stats => Request::Stats,
    };

    let stream = UnixStream::connect(&socket)
        .with_context(|| format!("нет соединения с {} — Grace.app запущен?", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut writer = stream.try_clone()?;
    write_message(&mut writer, &request)?;
    let response: Response = read_message(&mut BufReader::new(stream))?
        .context("рантайм закрыл соединение без ответа")?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    match response {
        Response::Pong { protocol, app, version } => println!("pong: {app} {version} (протокол {protocol})"),
        Response::Stats(s) => println!(
            "кадров {} · {:.1} FPS · {:.2} мс/кадр (CPU) · {}×{} · {}",
            s.frames, s.fps, s.frame_ms, s.drawable_width, s.drawable_height, s.device
        ),
        Response::Error { message } => bail!("ошибка рантайма: {message}"),
    }
    Ok(())
}
