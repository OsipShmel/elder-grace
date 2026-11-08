//! `cargo xtask oracle <команда> [игра]` — воспроизводимая среда эталона на GPTK.
//!
//! - `setup [игра]` — префикс Wine, compat-shim'ы, crash-probe, реестр, Windows-Steam.
//!   Идемпотентно: можно запускать повторно (например, после самообновления Steam).
//! - `steam [--ui]` — запустить клиент Steam в префиксе (по умолчанию без окна).
//! - `run [игра] [--no-hud] [--no-shim] [--no-overrides] [--label L]` — запустить игру из песочницы (Metal HUD включён,
//!   прокси d3d12 дампит шейдеры в `.local/shaders/<игра>/`, прокси dxgi пишет время кадра
//!   в `.local/frametimes/<игра>/<метка>.csv`).
//! - `kill` — остановить все процессы префикса.
//! - esync (`WINEESYNC=1`) включён по умолчанию для всех команд: без него каждое событие/мьютекс
//!   Windows — запрос к wineserver (трейс 2026-11-08: 22% времени главного потока игры).
//!   `--no-esync` — выключить (для A/B). Режим общий на префикс: wineserver, Steam и игра
//!   должны быть в одном режиме, при смене — `oracle kill`.
//! - `status` — процессы, вход в Steam, отчёты crash-probe.
//!
//! Что игра запускает и какой у неё App ID — берётся из `config/local.toml`
//! (`[games.<игра>] exe, steam_appid`), xtask об играх ничего не знает.
//! Логин в Steam пользователь делает сам: `oracle steam --ui`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use grace_core::config::{GameConfig, LocalConfig};

const GPTK_BIN: &str = "/Applications/Game Porting Toolkit.app/Contents/Resources/wine/bin";
const STEAM_SETUP_URL: &str = "https://cdn.akamai.steamstatic.com/client/installer/SteamSetup.exe";
const STEAM_DIR: &str = "drive_c/Program Files (x86)/Steam";
const SHIM_TARGET: &str = "target/x86_64-pc-windows-gnu/release";

/// Процессы, которым нужен compat-bcrypt (через него же грузятся остальные исправления).
const BCRYPT_EXES: &[&str] = &["steam.exe", "steamwebhelper.exe", "steamservice.exe", "steamcmd.exe"];

/// Прокси графического API, которые кладутся в песочницу игры.
const GAME_PROXIES: &[&str] = &["d3d12.dll", "dxgi.dll"];

struct Ctx {
    root: PathBuf,
    local: PathBuf,
    prefix: PathBuf,
    game_key: String,
    game: GameConfig,
}

/// Включать ли esync в запускаемых процессах Wine (см. описание модуля).
static ESYNC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn esync() -> bool {
    ESYNC.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn main(args: &[String]) -> Result<()> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let flag = |f: &str| args.iter().any(|a| a == f);
    ESYNC.store(!flag("--no-esync"), std::sync::atomic::Ordering::Relaxed);
    // Первый позиционный аргумент после команды (значение --label не считается).
    let game_key = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(i, a)| !a.starts_with("--") && args.get(i - 1).map(String::as_str) != Some("--label"))
        .map(|(_, a)| a.clone())
        .unwrap_or_else(|| "eldenring".into());
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
        _ => bail!("использование: cargo xtask oracle setup|steam [--ui]|run [--no-hud] [--no-shim] [--no-overrides]|kill|status [игра] [--no-esync]"),
    }
}

fn ctx(game_key: &str) -> Result<Ctx> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().context("корень workspace")?.to_owned();
    let cfg = LocalConfig::discover(&root)?;
    let game = cfg.games.get(game_key).with_context(|| format!("в config/local.toml нет [games.{game_key}]"))?.clone();
    let local = cfg.paths.local_dir.clone();
    Ok(Ctx { prefix: local.join("prefixes").join(game_key), root, local, game_key: game_key.into(), game })
}

fn wine(c: &Ctx, tool: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(Path::new(GPTK_BIN).join(tool));
    // GRACE_WINEDEBUG — чтобы включить каналы Wine (например, +debugstr) без правки кода.
    let debug = std::env::var("GRACE_WINEDEBUG").unwrap_or_else(|_| "-all".into());
    cmd.args(args).env("WINEPREFIX", &c.prefix).env("WINEDEBUG", debug);
    if esync() {
        cmd.env("WINEESYNC", "1");
    }
    cmd
}

/// Режим esync запущенного wineserver этого префикса (None — сервер не запущен).
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

/// Wine отказывается запускать процесс, если его режим esync не совпадает с сервером.
fn check_esync(c: &Ctx) -> Result<()> {
    if let Some(server) = server_esync(c)
        && server != esync()
    {
        let mode = |on: bool| if on { "с esync" } else { "без esync" };
        bail!("wineserver префикса запущен {}, а запрошено {} — сначала cargo xtask oracle kill", mode(server), mode(esync()));
    }
    Ok(())
}

fn run_ok(mut cmd: Command, what: &str) -> Result<()> {
    let status = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().with_context(|| what.to_string())?;
    if !status.success() {
        bail!("{what}: код {status}");
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
        bail!("нет GPTK. Поставить: brew trust gcenx/wine && brew install --cask gcenx/wine/game-porting-toolkit");
    }
    println!("[1/6] сборка shim'ов");
    crate::shim(true)?;

    println!("[2/6] префикс {}", c.prefix.display());
    if !c.prefix.join("drive_c").exists() {
        std::fs::create_dir_all(&c.prefix)?;
        run_ok(wine(c, "wine64", &["wineboot", "--init"]), "wineboot --init")?;
    }

    println!("[3/6] crash-probe и compat-realtime");
    let shims = c.root.join(SHIM_TARGET);
    copy(&shims.join("crash-probe.exe"), &c.prefix.join("drive_c/grace/crash-probe.exe"))?;
    copy(&shims.join("load-probe.exe"), &c.prefix.join("drive_c/grace/load-probe.exe"))?;
    let sys32 = c.prefix.join("drive_c/windows/system32");
    for name in ["api-ms-win-core-realtime-l1-1-1.dll", "api-ms-win-core-realtime-l1-1-2.dll"] {
        copy(&shims.join("api_ms_win_core_realtime.dll"), &sys32.join(name))?;
    }

    println!("[4/6] реестр");
    let mut reg = String::from("REGEDIT4\r\n\r\n");
    reg += "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\\AeDebug]\r\n";
    reg += "\"Debugger\"=\"C:\\\\grace\\\\crash-probe.exe %ld %ld\"\r\n\"Auto\"=\"1\"\r\n\r\n";
    reg += "[HKEY_CURRENT_USER\\Software\\Wine\\Mac Driver]\r\n\"CaptureDisplaysForFullscreen\"=\"n\"\r\n\r\n";
    for exe in BCRYPT_EXES {
        reg += &format!("[HKEY_CURRENT_USER\\Software\\Wine\\AppDefaults\\{exe}\\DllOverrides]\r\n\"bcrypt\"=\"native,builtin\"\r\n\r\n");
    }
    // Игре — ещё и наши прокси d3d12 (пайплайны, шейдеры) и dxgi (время кадра).
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
            run_ok(curl, "скачивание SteamSetup.exe")?;
        }
        let setup_win = format!("Z:{}", setup_exe.display()).replace('/', "\\");
        run_ok(wine(c, "wine64", &[&setup_win, "/S"]), "установка Steam")?;
    }

    println!("[6/6] shim'ы рядом с exe, песочница");
    install_shims(c)?;
    println!("готово. Дальше: cargo xtask oracle steam --ui (вход в Steam, один раз), затем cargo xtask oracle run {}", c.game_key);
    Ok(())
}

/// Кладёт compat-bcrypt рядом со всеми exe, которым он нужен, и steam_appid.txt в песочницу.
/// Дешёвая операция: вызывается перед каждым запуском (Steam мог обновиться).
fn install_shims(c: &Ctx) -> Result<()> {
    let bcrypt = c.root.join(SHIM_TARGET).join("bcrypt.dll");
    if !bcrypt.exists() {
        bail!("нет {} — сначала cargo xtask oracle setup", bcrypt.display());
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
        // Командная строка Wine-процесса не всегда содержит путь префикса (у игры — просто
        // "eldenring.exe"); считаем, что на машине один активный префикс эталона.
        .filter(|l| l.contains("wine64-preloader") && (l.contains(&prefix) || l.contains(".exe")))
        .map(|l| l.split(" HOMEBREW").next().unwrap_or(l).chars().take(160).collect())
        .collect()
}

fn steam_running(c: &Ctx) -> bool {
    prefix_processes(c).iter().any(|l| l.contains("Steam\\steam.exe") || l.contains("Steam/steam.exe"))
}

fn start_steam(c: &Ctx, ui: bool) -> Result<()> {
    if steam_running(c) {
        println!("Steam уже запущен");
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
    cmd.stdout(log.try_clone()?).stderr(log).spawn().context("запуск Steam")?;
    println!("Steam запускается{} (лог: {})", if ui { " с окном" } else { " в фоне" }, logs.join("steam.log").display());
    Ok(())
}

struct RunOpts {
    hud: bool,
    shim: bool,
    overrides: bool,
    /// Метка прогона для журнала кадров (`.local/frametimes/<игра>/<метка>.csv`).
    label: Option<String>,
}

fn run(c: &Ctx, opts: RunOpts) -> Result<()> {
    let RunOpts { hud, shim, overrides, label } = opts;
    check_esync(c)?;
    println!("esync: {}", if esync() { "включён" } else { "выключен (--no-esync)" });
    install_shims(c)?;
    // Без shim: убираем наш d3d12.dll из песочницы — override native,builtin откатывается
    // на builtin D3DMetal. dxgi-прокси оставляем: он только меряет время кадра,
    // и сравнение «с/без» тогда честное — меряется одинаково.
    let proxy = c.game.sandbox_dir.join("d3d12.dll");
    if !shim && proxy.exists() {
        std::fs::remove_file(&proxy)?;
        println!("shim d3d12 отключён для этого запуска (журнал кадров остаётся)");
    }
    let exe = c.game.exe.clone().with_context(|| format!("в [games.{}] нет exe", c.game_key))?;
    if !steam_running(c) {
        start_steam(c, false)?;
        println!("жду 25 с, пока Steam войдёт в сеть…");
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
    // Каталог дампа шейдеров для прокси d3d12: путь macOS через диск Z: Wine.
    let dump = c.local.join("shaders").join(&c.game_key);
    std::fs::create_dir_all(&dump)?;
    cmd.env("GRACE_SHADER_DUMP", format!("Z:{}", dump.display()).replace('/', "\\"));
    // Подмены шейдеров (cargo xtask overrides): shim подставляет их вместо байткода игры.
    let ov = c.local.join("shaders").join(format!("{}-override", c.game_key));
    if overrides && ov.exists() {
        let n = std::fs::read_dir(&ov).map(|d| d.count()).unwrap_or(0);
        cmd.env("GRACE_SHADER_OVERRIDE", format!("Z:{}", ov.display()).replace('/', "\\"));
        println!("подмен шейдеров: {n} ({})", ov.display());
    }
    // Журнал времени кадра (dxgi-прокси).
    let label = label.unwrap_or_else(|| {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("run-{secs}")
    });
    let frames_dir = c.local.join("frametimes").join(&c.game_key);
    std::fs::create_dir_all(&frames_dir)?;
    let frames_csv = frames_dir.join(format!("{label}.csv"));
    cmd.env("GRACE_FRAMETIME_LOG", format!("Z:{}", frames_csv.display()).replace('/', "\\"));
    cmd.stdout(log.try_clone()?).stderr(log).spawn().context("запуск игры")?;
    println!("{exe} запущен из {}", c.game.sandbox_dir.display());
    println!("журнал кадров: {} (отчёт: cargo xtask bench {})", frames_csv.display(), frames_csv.display());
    Ok(())
}

fn status(c: &Ctx) -> Result<()> {
    println!("префикс: {}", c.prefix.display());
    let procs = prefix_processes(c);
    println!("процессов Wine: {}", procs.len());
    match server_esync(c) {
        Some(on) => println!("wineserver: {}", if on { "esync включён" } else { "esync выключен" }),
        None => println!("wineserver: не запущен"),
    }
    for p in procs.iter().filter(|p| p.contains(".exe") && !p.contains("system32")) {
        println!("  {}", p.split("preloader ").nth(1).unwrap_or(p));
    }
    let conn = c.prefix.join(STEAM_DIR).join("logs/connection_log.txt");
    if let Ok(text) = std::fs::read_to_string(&conn) {
        let last = text.lines().rev().find(|l| l.contains("Logged On") || l.contains("Logged Off"));
        let state = last.map_or("нет данных", |l| if l.contains("Logged On") { "в сети (Logged On)" } else { "не вошёл" });
        println!("Steam: {state}");
    }
    let crashes = c.prefix.join("drive_c/grace-crash");
    let n = std::fs::read_dir(&crashes).map(|d| d.count()).unwrap_or(0);
    println!("отчётов crash-probe: {n} ({})", crashes.display());
    Ok(())
}
