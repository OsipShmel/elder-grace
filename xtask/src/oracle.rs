//! `cargo xtask oracle <command> [game]`: a reproducible GPTK oracle environment.
//!
//! - `setup [game]`: Wine prefix, compat shims, crash-probe, registry, Windows Steam.
//!   Idempotent: safe to run again (for example, after Steam updates itself).
//! - `steam [--ui]`: start the Steam client in the prefix (windowless by default).
//! - `run [game] [--no-hud] [--no-shim] [--no-overrides] [--label L]`: run the game from the sandbox (Metal HUD on,
//!   the d3d12 proxy dumps shaders into `.local/shaders/<game>/`, the dxgi proxy logs frame times
//!   to `.local/frametimes/<game>/<label>.csv`).
//! - `kill`: stop every process in the prefix.
//! - `shot <out.png>`: screenshot of the game window (title from `GRACE_WINDOW`, else `window_title` of the game config),
//!   even when it is not in front.
//! - esync (`WINEESYNC=1`) is on by default for every command: without it every Windows event/mutex
//!   is a wineserver round trip (trace 2026-11-08: 22% of the game's main thread time).
//!   `--no-esync` turns it off (for A/B). The mode is per prefix: wineserver, Steam and the game
//!   must use the same mode; after switching, run `oracle kill`.
//! - `status`: processes, Steam login state, crash-probe reports.
//!
//! What the game runs and its App ID come from `config/local.toml`
//! (`[games.<game>] exe, steam_appid, window_title`); xtask knows nothing about games.
//! The user logs into Steam themselves: `oracle steam --ui`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use grace_core::config::{GameConfig, LocalConfig};

const GPTK_BIN: &str = "/Applications/Game Porting Toolkit.app/Contents/Resources/wine/bin";
const STEAM_SETUP_URL: &str = "https://cdn.akamai.steamstatic.com/client/installer/SteamSetup.exe";
const STEAM_DIR: &str = "drive_c/Program Files (x86)/Steam";
const SHIM_TARGET: &str = "target/x86_64-pc-windows-gnu/release";

/// Processes that need compat-bcrypt (the other fixes are loaded through it as well).
const BCRYPT_EXES: &[&str] = &["steam.exe", "steamwebhelper.exe", "steamservice.exe", "steamcmd.exe"];

/// Graphics API proxies that go into the game sandbox.
const GAME_PROXIES: &[&str] = &["d3d12.dll", "dxgi.dll"];

struct Ctx {
    root: PathBuf,
    local: PathBuf,
    prefix: PathBuf,
    game_key: String,
    game: GameConfig,
}

/// Whether esync is enabled for launched Wine processes (see the module docs).
static ESYNC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn esync() -> bool {
    ESYNC.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn main(args: &[String]) -> Result<()> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let flag = |f: &str| args.iter().any(|a| a == f);
    ESYNC.store(!flag("--no-esync"), std::sync::atomic::Ordering::Relaxed);
    // First positional argument after the command (the value of --label does not count).
    let game_key = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(i, a)| !a.starts_with("--") && args.get(i - 1).map(String::as_str) != Some("--label"))
        .map(|(_, a)| a.clone())
        .map_or_else(crate::default_game, Ok)?;
    match cmd {
        "setup" => setup(&ctx(&game_key)?),
        "steam" => {
            let c = ctx(&game_key)?;
            check_esync(&c)?;
            install_shims(&c)?;
            start_steam(&c, flag("--ui"))
        }
        "run" => {
            let label = args.iter().position(|a| a == "--label").and_then(|i| args.get(i + 1)).cloned();
            run(&ctx(&game_key)?, RunOpts { hud: !flag("--no-hud"), shim: !flag("--no-shim"), overrides: !flag("--no-overrides"), label })
        }
        "kill" => {
            wine(&ctx(&game_key)?, "wineserver", &["-k"]).status()?;
            Ok(())
        }
        "status" => status(&ctx(&game_key)?),
        "shot" => {
            let out = args.get(1).context("usage: cargo xtask oracle shot <out.png>")?;
            shot(Path::new(out), &crate::default_game()?)
        }
        _ => bail!("usage: cargo xtask oracle setup|steam [--ui]|run [--no-hud] [--no-shim] [--no-overrides]|kill|status|shot <png> [game] [--no-esync]"),
    }
}

fn ctx(game_key: &str) -> Result<Ctx> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().context("workspace root")?.to_owned();
    let cfg = LocalConfig::discover(&root)?;
    let game = cfg.games.get(game_key).with_context(|| format!("config/local.toml has no [games.{game_key}]"))?.clone();
    let local = cfg.paths.local_dir.clone();
    Ok(Ctx { prefix: local.join("prefixes").join(game_key), root, local, game_key: game_key.into(), game })
}

fn wine(c: &Ctx, tool: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(Path::new(GPTK_BIN).join(tool));
    // GRACE_WINEDEBUG enables Wine channels (for example, +debugstr) without code changes.
    let debug = std::env::var("GRACE_WINEDEBUG").unwrap_or_else(|_| "-all".into());
    cmd.args(args).env("WINEPREFIX", &c.prefix).env("WINEDEBUG", debug);
    if esync() {
        cmd.env("WINEESYNC", "1");
    }
    cmd
}

/// esync mode of the running wineserver of this prefix (None: the server is not running).
fn server_esync(c: &Ctx) -> Option<bool> {
    let pids = Command::new("pgrep").args(["-f", "bin/wineserver"]).output().ok()?;
    let prefix = format!("WINEPREFIX={}", c.prefix.display());
    for pid in String::from_utf8_lossy(&pids.stdout).split_whitespace() {
        let env = Command::new("ps").args(["-wwE", "-o", "command=", "-p", pid]).output().ok()?;
        let env = String::from_utf8_lossy(&env.stdout).into_owned();
        if env.split_whitespace().any(|w| w == prefix) {
            return Some(env.split_whitespace().any(|w| w == "WINEESYNC=1"));
        }
    }
    None
}

/// Wine refuses to start a process whose esync mode differs from the server's.
fn check_esync(c: &Ctx) -> Result<()> {
    if let Some(server) = server_esync(c)
        && server != esync()
    {
        let mode = |on: bool| if on { "with esync" } else { "without esync" };
        bail!("the prefix wineserver is running {}, but {} was requested: run cargo xtask oracle kill first", mode(server), mode(esync()));
    }
    Ok(())
}

fn run_ok(mut cmd: Command, what: &str) -> Result<()> {
    let status = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().with_context(|| what.to_string())?;
    if !status.success() {
        bail!("{what}: exit code {status}");
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::copy(from, to).with_context(|| format!("{} → {}", from.display(), to.display()))?;
    Ok(())
}

fn setup(c: &Ctx) -> Result<()> {
    if !Path::new(GPTK_BIN).join("wine64").exists() {
        bail!("GPTK not found. Install it: brew trust gcenx/wine && brew install --cask gcenx/wine/game-porting-toolkit");
    }
    println!("[1/6] building shims");
    crate::shim(true)?;

    println!("[2/6] prefix {}", c.prefix.display());
    if !c.prefix.join("drive_c").exists() {
        std::fs::create_dir_all(&c.prefix)?;
        run_ok(wine(c, "wine64", &["wineboot", "--init"]), "wineboot --init")?;
    }

    println!("[3/6] crash-probe and compat-realtime");
    let shims = c.root.join(SHIM_TARGET);
    copy(&shims.join("crash-probe.exe"), &c.prefix.join("drive_c/grace/crash-probe.exe"))?;
    copy(&shims.join("load-probe.exe"), &c.prefix.join("drive_c/grace/load-probe.exe"))?;
    let sys32 = c.prefix.join("drive_c/windows/system32");
    for name in ["api-ms-win-core-realtime-l1-1-1.dll", "api-ms-win-core-realtime-l1-1-2.dll"] {
        copy(&shims.join("api_ms_win_core_realtime.dll"), &sys32.join(name))?;
    }

    println!("[4/6] registry");
    let mut reg = String::from("REGEDIT4\r\n\r\n");
    reg += "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\\AeDebug]\r\n";
    reg += "\"Debugger\"=\"C:\\\\grace\\\\crash-probe.exe %ld %ld\"\r\n\"Auto\"=\"1\"\r\n\r\n";
    reg += "[HKEY_CURRENT_USER\\Software\\Wine\\Mac Driver]\r\n\"CaptureDisplaysForFullscreen\"=\"n\"\r\n\r\n";
    for exe in BCRYPT_EXES {
        reg += &format!("[HKEY_CURRENT_USER\\Software\\Wine\\AppDefaults\\{exe}\\DllOverrides]\r\n\"bcrypt\"=\"native,builtin\"\r\n\r\n");
    }
    // The game also gets our d3d12 (pipelines, shaders) and dxgi (frame time) proxies.
    if let Some(exe) = &c.game.exe {
        reg += &format!(
            "[HKEY_CURRENT_USER\\Software\\Wine\\AppDefaults\\{exe}\\DllOverrides]\r\n\"bcrypt\"=\"native,builtin\"\r\n\"d3d12\"=\"native,builtin\"\r\n\"dxgi\"=\"native,builtin\"\r\n\r\n"
        );
    }
    let reg_file = c.prefix.join("drive_c/grace/grace-oracle.reg");
    std::fs::write(&reg_file, reg)?;
    run_ok(wine(c, "wine64", &["regedit", "/S", "C:\\grace\\grace-oracle.reg"]), "regedit")?;

    println!("[5/6] Windows-Steam");
    let steam = c.prefix.join(STEAM_DIR);
    if !steam.join("steam.exe").exists() {
        let setup_exe = c.local.join("downloads/SteamSetup.exe");
        if !setup_exe.exists() {
            std::fs::create_dir_all(setup_exe.parent().context("downloads")?)?;
            let mut curl = Command::new("curl");
            curl.arg("-fsSLo").arg(&setup_exe).arg(STEAM_SETUP_URL);
            run_ok(curl, "downloading SteamSetup.exe")?;
        }
        let setup_win = format!("Z:{}", setup_exe.display()).replace('/', "\\");
        run_ok(wine(c, "wine64", &[&setup_win, "/S"]), "installing Steam")?;
    }

    println!("[6/6] shims next to the exe, sandbox");
    install_shims(c)?;
    println!("done. Next: cargo xtask oracle steam --ui (Steam login, once), then cargo xtask oracle run {}", c.game_key);
    Ok(())
}

/// Puts compat-bcrypt next to every exe that needs it, and steam_appid.txt into the sandbox.
/// Cheap: runs before every launch (Steam may have updated).
fn install_shims(c: &Ctx) -> Result<()> {
    let bcrypt = c.root.join(SHIM_TARGET).join("bcrypt.dll");
    if !bcrypt.exists() {
        bail!("{} is missing: run cargo xtask oracle setup first", bcrypt.display());
    }
    let steam = c.prefix.join(STEAM_DIR);
    for dir in [steam.clone(), steam.join("bin/cef/cef.win64"), c.prefix.join("drive_c/steamcmd"), c.game.sandbox_dir.clone()] {
        if dir.exists() {
            copy(&bcrypt, &dir.join("bcrypt.dll"))?;
        }
    }
    for proxy in GAME_PROXIES {
        let built = c.root.join(SHIM_TARGET).join(proxy);
        if built.exists() && c.game.sandbox_dir.exists() {
            copy(&built, &c.game.sandbox_dir.join(proxy))?;
        }
    }
    if let Some(id) = c.game.steam_appid {
        std::fs::write(c.game.sandbox_dir.join("steam_appid.txt"), id.to_string())?;
    }
    Ok(())
}

fn prefix_processes(c: &Ctx) -> Vec<String> {
    let out = Command::new("ps").args(["-axo", "pid=,command="]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let prefix = c.prefix.to_string_lossy().into_owned();
    out.lines()
        // A Wine process command line does not always contain the prefix path (for a game it is
        // just the exe name); we assume a single active oracle prefix on the machine.
        .filter(|l| l.contains("wine64-preloader") && (l.contains(&prefix) || l.contains(".exe")))
        .map(|l| l.split(" HOMEBREW").next().unwrap_or(l).chars().take(160).collect())
        .collect()
}

fn steam_running(c: &Ctx) -> bool {
    prefix_processes(c).iter().any(|l| l.contains("Steam\\steam.exe") || l.contains("Steam/steam.exe"))
}

fn start_steam(c: &Ctx, ui: bool) -> Result<()> {
    if steam_running(c) {
        println!("Steam is already running");
        return Ok(());
    }
    let logs = c.local.join("logs");
    std::fs::create_dir_all(&logs)?;
    let log = std::fs::File::create(logs.join("steam.log"))?;
    let exe = c.prefix.join(STEAM_DIR).join("steam.exe");
    let mut cmd = wine(c, "wine64", &[]);
    cmd.arg(&exe);
    if !ui {
        cmd.arg("-silent");
    }
    cmd.stdout(log.try_clone()?).stderr(log).spawn().context("starting Steam")?;
    println!("Steam is starting{} (log: {})", if ui { " with a window" } else { " in the background" }, logs.join("steam.log").display());
    Ok(())
}

struct RunOpts {
    hud: bool,
    shim: bool,
    overrides: bool,
    /// Run label for the frame log (`.local/frametimes/<game>/<label>.csv`).
    label: Option<String>,
}

fn run(c: &Ctx, opts: RunOpts) -> Result<()> {
    let RunOpts { hud, shim, overrides, label } = opts;
    check_esync(c)?;
    println!("esync: {}", if esync() { "on" } else { "off (--no-esync)" });
    install_shims(c)?;
    // Without the shim: remove our d3d12.dll from the sandbox, so the native,builtin override falls back
    // to builtin D3DMetal. The dxgi proxy stays: it only measures frame time,
    // so a with/without comparison is fair because both are measured the same way.
    let proxy = c.game.sandbox_dir.join("d3d12.dll");
    if !shim && proxy.exists() {
        std::fs::remove_file(&proxy)?;
        println!("d3d12 shim disabled for this run (the frame log stays)");
    }
    let exe = c.game.exe.clone().with_context(|| format!("[games.{}] has no exe", c.game_key))?;
    if !steam_running(c) {
        start_steam(c, false)?;
        println!("waiting 25 s for Steam to go online…");
        std::thread::sleep(std::time::Duration::from_secs(25));
    }
    let logs = c.local.join("logs");
    std::fs::create_dir_all(&logs)?;
    let log = std::fs::File::create(logs.join(format!("{}.log", c.game_key)))?;
    let mut cmd = wine(c, "wine64", &[&exe]);
    cmd.current_dir(&c.game.sandbox_dir);
    if hud {
        cmd.env("MTL_HUD_ENABLED", "1");
    }
    // Shader dump directory for the d3d12 proxy: the macOS path through Wine's Z: drive.
    let dump = c.local.join("shaders").join(&c.game_key);
    std::fs::create_dir_all(&dump)?;
    cmd.env("GRACE_SHADER_DUMP", format!("Z:{}", dump.display()).replace('/', "\\"));
    // Shader overrides (cargo xtask overrides): the shim substitutes them for the game's bytecode.
    let ov = c.local.join("shaders").join(format!("{}-override", c.game_key));
    if overrides && ov.exists() {
        let n = std::fs::read_dir(&ov).map(|d| d.count()).unwrap_or(0);
        cmd.env("GRACE_SHADER_OVERRIDE", format!("Z:{}", ov.display()).replace('/', "\\"));
        println!("shader overrides: {n} ({})", ov.display());
    }
    // Frame-time log (dxgi proxy).
    let label = label.unwrap_or_else(|| {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("run-{secs}")
    });
    let frames_dir = c.local.join("frametimes").join(&c.game_key);
    std::fs::create_dir_all(&frames_dir)?;
    let frames_csv = frames_dir.join(format!("{label}.csv"));
    cmd.env("GRACE_FRAMETIME_LOG", format!("Z:{}", frames_csv.display()).replace('/', "\\"));
    // Live mirror (dxgi proxy): a fresh record each run, so a host never reads a previous run's frames.
    let mirror = grace_live::mirror_file(&c.local, &c.game_key);
    std::fs::create_dir_all(mirror.parent().context("mirror dir")?)?;
    let _ = std::fs::remove_file(&mirror);
    cmd.env("GRACE_MIRROR", format!("Z:{}", mirror.display()).replace('/', "\\"));
    cmd.stdout(log.try_clone()?).stderr(log).spawn().context("starting the game")?;
    println!("{exe} started from {}", c.game.sandbox_dir.display());
    println!("live mirror: {}", mirror.display());
    println!("frame log: {} (report: cargo xtask bench {})", frames_csv.display(), frames_csv.display());
    Ok(())
}

/// Screenshot of the game window via `screencapture -l <window number>` (number found by window title).
fn shot(out: &Path, game_key: &str) -> Result<()> {
    let title = match std::env::var("GRACE_WINDOW") {
        Ok(t) => t,
        Err(_) => ctx(game_key)?.game.window_title.with_context(|| format!("set GRACE_WINDOW or window_title in [games.{game_key}]"))?,
    };
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/scripts/window-id.swift");
    let id = Command::new("xcrun").arg("swift").arg(&script).arg(&title).output().context("xcrun swift")?;
    let id = String::from_utf8_lossy(&id.stdout).trim().to_owned();
    if id.is_empty() {
        bail!("window \"{title}\" not found: is the game running?");
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let status = Command::new("screencapture").args(["-x", "-o", &format!("-l{id}")]).arg(out).status()?;
    if !status.success() {
        bail!("screencapture failed");
    }
    println!("window {id} screenshot: {}", out.display());
    Ok(())
}

fn status(c: &Ctx) -> Result<()> {
    println!("prefix: {}", c.prefix.display());
    let procs = prefix_processes(c);
    println!("Wine processes: {}", procs.len());
    match server_esync(c) {
        Some(on) => println!("wineserver: {}", if on { "esync on" } else { "esync off" }),
        None => println!("wineserver: not running"),
    }
    for p in procs.iter().filter(|p| p.contains(".exe") && !p.contains("system32")) {
        println!("  {}", p.split("preloader ").nth(1).unwrap_or(p));
    }
    let conn = c.prefix.join(STEAM_DIR).join("logs/connection_log.txt");
    if let Ok(text) = std::fs::read_to_string(&conn) {
        let last = text.lines().rev().find(|l| l.contains("Logged On") || l.contains("Logged Off"));
        let state = last.map_or("no data", |l| if l.contains("Logged On") { "online (Logged On)" } else { "not logged in" });
        println!("Steam: {state}");
    }
    let crashes = c.prefix.join("drive_c/grace-crash");
    let n = std::fs::read_dir(&crashes).map(|d| d.count()).unwrap_or(0);
    println!("crash-probe reports: {n} ({})", crashes.display());
    Ok(())
}
