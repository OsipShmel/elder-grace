//! `cargo xtask bundle [--debug]`: build `target/Grace.app`.
//! `cargo xtask shim [--release]`: build the shims for Windows x86_64.
//! `cargo xtask oracle …`: the GPTK oracle environment (see oracle.rs).
//! `cargo xtask shaders [game]`: shader corpus: summary -> DXIL->metallib -> check in Metal.
//! `cargo xtask overrides [game] [--reload]`: build shader overrides from `../grace-<game>/shaders/overrides/`
//!   (`.hlsl` is a replacement, `.irpatch` a patch of the original DXIL; see overrides.rs);
//!   `--reload` reloads them in the running game right away (d3d12 shim, TCP 47322).
//! `cargo xtask shader <hash>.<stage> [--ir]`: name, signatures, resources of a dumped shader; `--ir` disassembles into `.local`.
//! `cargo xtask bench <log|label> [<log2|label2>] [--skip sec] [--game game]`: frame-time report.
//! `cargo xtask trace [--seconds N] [--label L] [--game game] [--template T] [--instrument I]…`: trace the running game;
//!   Metal System Trace + CPU/GPU verdict by default; `--template "System Trace"` shows thread states (waits).
//!
//! `game` defaults to `GRACE_GAME`, or to the only `[games.*]` entry in `config/local.toml`.

mod oracle;
mod overrides;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    match args.first().map(String::as_str) {
        Some("bundle") => bundle(!flag("--debug")),
        Some("shim") => shim(flag("--release")),
        Some("oracle") => oracle::main(&args[1..]),
        Some("shaders") => shaders(&args.get(1).cloned().map_or_else(default_game, Ok)?),
        Some("overrides") => {
            let game = args.get(1).filter(|a| !a.starts_with("--")).cloned().map_or_else(default_game, Ok)?;
            overrides::build(&project_dir()?, &game)?;
            if flag("--reload") { reload_overrides() } else { Ok(()) }
        }
        Some("shader") => {
            let id = args.get(1).filter(|a| !a.starts_with("--")).context("usage: cargo xtask shader <hash>.<stage> [--ir] [--game game]")?;
            let game = args.iter().position(|a| a == "--game").and_then(|i| args.get(i + 1)).cloned().map_or_else(default_game, Ok)?;
            overrides::show(&project_dir()?, &game, id, flag("--ir"))
        }
        Some("bench") => bench(&args[1..]),
        Some("trace") => trace(&args[1..]),
        _ => bail!("usage: cargo xtask bundle [--debug] | shim [--release] | oracle … | shaders | overrides | shader | bench | trace"),
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask lives in the workspace root").to_owned()
}

/// Shared project folder (`ProjectPort/`): game repositories and `.local/` live next to the engine.
fn project_dir() -> Result<PathBuf> {
    Ok(root().parent().context("project folder above the engine checkout")?.to_owned())
}

/// Game to work with when none is given: `GRACE_GAME`, else the only `[games.*]` entry of the local config.
pub(crate) fn default_game() -> Result<String> {
    if let Ok(game) = std::env::var("GRACE_GAME") {
        return Ok(game);
    }
    let cfg = grace_core::config::LocalConfig::discover(&root())?;
    let mut games = cfg.games.keys();
    match (games.next(), games.next()) {
        (Some(game), None) => Ok(game.clone()),
        (None, _) => bail!("config/local.toml has no [games.*] entries"),
        _ => bail!("several games in config/local.toml: pass the game explicitly or set GRACE_GAME"),
    }
}

fn cargo(args: &[&str]) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo).args(args).current_dir(root()).status()?;
    if !status.success() {
        bail!("cargo {} failed", args.join(" "));
    }
    Ok(())
}

fn bundle(release: bool) -> Result<()> {
    let profile = if release { "release" } else { "debug" };
    let mut build = vec!["build", "-p", "grace-app"];
    if release {
        build.push("--release");
    }
    cargo(&build)?;

    let target = root().join("target");
    let app = target.join("Grace.app");
    let macos = app.join("Contents/MacOS");
    if app.exists() {
        std::fs::remove_dir_all(&app)?;
    }
    std::fs::create_dir_all(&macos)?;
    std::fs::copy(target.join(profile).join("grace-app"), macos.join("grace-app")).context("copying the binary")?;
    std::fs::write(app.join("Contents/Info.plist"), info_plist())?;

    // Ad-hoc signature: without it macOS on Apple Silicon will not launch the built bundle.
    let status = Command::new("codesign").args(["--force", "--sign", "-"]).arg(&app).status()?;
    if !status.success() {
        bail!("codesign failed");
    }
    println!("{}", app.display());
    Ok(())
}

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Elder Grace</string>
    <key>CFBundleDisplayName</key><string>Elder Grace</string>
    <key>CFBundleIdentifier</key><string>dev.eldergrace.app</string>
    <key>CFBundleExecutable</key><string>grace-app</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>{v}</string>
    <key>CFBundleVersion</key><string>{v}</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.games</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>GCSupportsControllerUserInteraction</key><true/>
</dict>
</plist>
"#,
        v = env!("CARGO_PKG_VERSION")
    )
}

pub(crate) fn shim(release: bool) -> Result<()> {
    let mut build = vec!["build", "-p", "graphics-proxy", "-p", "dxgi-proxy", "-p", "load-probe", "-p", "compat-bcrypt", "-p", "crash-probe", "-p", "compat-realtime", "--target", "x86_64-pc-windows-gnu"];
    if release {
        build.push("--release");
    }
    cargo(&build)?;
    let profile = if release { "release" } else { "debug" };
    let dll = root().join("target/x86_64-pc-windows-gnu").join(profile).join("graphics_proxy.dll");
    println!("{}", dll.display());
    Ok(())
}

/// Shader pipeline over the corpus that the d3d12 shim dumps into `.local/shaders/<game>/`.
/// DXIL -> metallib: Metal Shader Converter (grace-irconv, under Rosetta).
/// DXBC SM4/5 → SPIR-V (vkd3d-compiler) → MSL (SPIRV-Cross) → metallib (xcrun metal).
/// All metallibs go into `.local/shaders/<game>-metal/` and are checked in Metal natively.
fn shaders(game: &str) -> Result<()> {
    let local = project_dir()?.join(".local");
    let corpus = local.join("shaders").join(game);
    let metal = local.join("shaders").join(format!("{game}-metal"));
    let work = local.join("shaders").join(format!("{game}-msl"));
    std::fs::create_dir_all(&metal)?;
    std::fs::create_dir_all(&work)?;
    let (corpus_s, metal_s) = (corpus.to_string_lossy().into_owned(), metal.to_string_lossy().into_owned());

    println!("== corpus summary");
    cargo(&["run", "-q", "--release", "-p", "grace-shader", "--", "inventory", &corpus_s])?;

    println!("== DXIL → metallib (Metal Shader Converter from GPTK, under Rosetta)");
    cargo(&["build", "-q", "--release", "-p", "grace-irconv", "--target", "x86_64-apple-darwin"])?;
    let status = Command::new(root().join("target/x86_64-apple-darwin/release/grace-irconv")).args([&corpus_s, &metal_s]).status()?;
    if !status.success() {
        bail!("grace-irconv failed");
    }

    println!("== DXBC → SPIR-V → MSL → metallib (vkd3d + SPIRV-Cross + xcrun metal)");
    let vkd3d = local.join("tools/bin/vkd3d-compiler");
    if !vkd3d.exists() {
        bail!("{} is missing: building vkd3d is described in docs/ARCHITECTURE.md (shader pipeline)", vkd3d.display());
    }
    let (mut ok, mut failed) = (0, 0);
    let mut files: Vec<_> = std::fs::read_dir(&corpus)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    for path in files.iter().filter(|p| p.extension().and_then(|e| e.to_str()) == Some("dxbc")) {
        let data = std::fs::read(path)?;
        let Ok(Some(info)) = grace_shader::inspect(&data) else { continue };
        if info.bytecode != grace_shader::Bytecode::Dxbc {
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let out = metal.join(format!("{stem}.metallib"));
        if out.exists() {
            ok += 1;
            continue;
        }
        let (spv, msl, air) = (work.join(format!("{stem}.spv")), work.join(format!("{stem}.metal")), work.join(format!("{stem}.air")));
        let steps: [(&str, Vec<std::ffi::OsString>); 4] = [
            ("vkd3d", vec!["-x".into(), "dxbc-tpf".into(), "-b".into(), "spirv-binary".into(), "-o".into(), spv.clone().into(), path.clone().into()]),
            ("spirv-cross", vec!["--msl".into(), "--msl-version".into(), "30000".into(), spv.clone().into(), "--output".into(), msl.clone().into()]),
            ("metal", vec!["-sdk".into(), "macosx".into(), "metal".into(), "-c".into(), msl.clone().into(), "-o".into(), air.clone().into()]),
            ("metallib", vec!["-sdk".into(), "macosx".into(), "metallib".into(), air.clone().into(), "-o".into(), out.clone().into()]),
        ];
        let mut good = true;
        for (tool, args) in steps {
            let program: std::ffi::OsString = match tool {
                "vkd3d" => vkd3d.clone().into(),
                "spirv-cross" => "spirv-cross".into(),
                _ => "xcrun".into(),
            };
            let res = Command::new(&program).args(&args).output()?;
            if !res.status.success() {
                eprintln!("{stem}: {tool}: {}", String::from_utf8_lossy(&res.stderr).lines().next().unwrap_or("error"));
                good = false;
                break;
            }
        }
        if good { ok += 1 } else { failed += 1 }
    }
    println!("DXBC → metallib: {ok} ok, {failed} failed");

    println!("== check in Metal (native)");
    cargo(&["run", "-q", "--release", "-p", "grace-shader", "--", "metallib", &metal_s])
}

/// Reloads overrides in the running game through the d3d12 shim control channel.
fn reload_overrides() -> Result<()> {
    use std::io::BufReader;
    let addr = "127.0.0.1:47322";
    let stream = std::net::TcpStream::connect(addr).with_context(|| format!("cannot connect to {addr}: is the game running with the shim?"))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(60)))?;
    let mut w = stream.try_clone()?;
    grace_proto::write_message(&mut w, &grace_proto::Request::Overrides { op: grace_proto::OverrideOp::Reload })?;
    match grace_proto::read_message::<_, grace_proto::Response>(&mut BufReader::new(stream))? {
        Some(grace_proto::Response::Overrides(o)) => {
            println!("{}", o.last_reload);
            println!("overrides {} · PSO pairs {}", if o.enabled { "ON" } else { "off" }, o.pairs);
            Ok(())
        }
        Some(grace_proto::Response::Error { message }) => bail!("shim: {message}"),
        other => bail!("unexpected shim reply: {other:?}"),
    }
}

/// Report over frame logs. The argument is a CSV path or a run label
/// (`.local/frametimes/<game>/<label>.csv`).
fn bench(args: &[String]) -> Result<()> {
    let game = args.iter().position(|a| a == "--game").and_then(|i| args.get(i + 1)).cloned().map_or_else(default_game, Ok)?;
    let dir = project_dir()?.join(".local/frametimes").join(game);
    let mut pass: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--game" => {
                it.next();
            }
            "--skip" => {
                pass.push(a.clone());
                if let Some(v) = it.next() {
                    pass.push(v.clone());
                }
            }
            other => {
                let p = PathBuf::from(other);
                let path = if p.exists() { p } else { dir.join(format!("{other}.csv")) };
                pass.push(path.to_string_lossy().into_owned());
            }
        }
    }
    let mut cmd = vec!["run".to_string(), "-q".into(), "--release".into(), "-p".into(), "grace-bench".into(), "--".into()];
    cmd.extend(pass);
    cargo(&cmd.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Metal System Trace of the game running under the oracle, with analysis: CPU- or GPU-bound.
fn trace(args: &[String]) -> Result<()> {
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let game = opt("--game").map_or_else(default_game, Ok)?;
    let seconds = opt("--seconds").unwrap_or_else(|| "15".into());
    let label = opt("--label").unwrap_or_else(|| {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("trace-{secs}")
    });
    let project = project_dir()?;
    let cfg = grace_core::config::LocalConfig::discover(&root())?;
    let exe = cfg.games.get(&game).and_then(|g| g.exe.clone()).with_context(|| format!("[games.{game}] has no exe"))?;

    // The game process under Wine: wine64-preloader with the exe name on its command line.
    let ps = Command::new("ps").args(["-axo", "pid=,command="]).output()?;
    let pid = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .find(|l| l.contains("wine64-preloader") && l.contains(exe.as_str()))
        .and_then(|l| l.split_whitespace().next().map(str::to_owned))
        .with_context(|| format!("{exe} is not running: cargo xtask oracle run"))?;

    let dir = project.join(".local/traces").join(&game);
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{label}.trace"));
    if out.exists() {
        bail!("{} already exists: pick another --label", out.display());
    }
    let template = opt("--template").unwrap_or_else(|| "Metal System Trace".into());
    let instruments: Vec<&String> = args.iter().enumerate().filter(|(_, a)| *a == "--instrument").filter_map(|(i, _)| args.get(i + 1)).collect();
    println!("recording \"{template}\": {exe} (pid {pid}), {seconds} s -> {}", out.display());
    let mut cmd = Command::new("xcrun");
    cmd.args(["xctrace", "record", "--template", &template, "--attach", &pid, "--time-limit", &format!("{seconds}s", seconds = seconds.trim_end_matches('s'))]);
    for i in &instruments {
        cmd.args(["--instrument", i.as_str()]);
    }
    let status = cmd.arg("--output").arg(&out).stdout(std::process::Stdio::null()).status()?;
    if !status.success() {
        bail!("xctrace record failed");
    }
    if template != "Metal System Trace" {
        println!("trace recorded; the verdict is computed only for Metal System Trace (tables: xcrun xctrace export --input {} --toc)", out.display());
        return Ok(());
    }
    let out_s = out.to_string_lossy().into_owned();
    cargo(&["run", "-q", "--release", "-p", "grace-bench", "--", "trace", &out_s, "--pid", &pid])
}
