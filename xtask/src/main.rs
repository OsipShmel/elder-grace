//! `cargo xtask bundle [--debug]` — собрать `target/Grace.app`.
//! `cargo xtask shim [--release]` — собрать shim под Windows x86_64.
//! `cargo xtask oracle …` — среда эталона на GPTK (см. oracle.rs).
//! `cargo xtask shaders [игра]` — корпус шейдеров: сводка → DXIL→metallib → проверка в Metal.
//! `cargo xtask overrides [игра] [--reload]` — собрать подмены шейдеров из `../grace-<игра>/shaders/overrides/`
//!   (`.hlsl` — замена, `.irpatch` — правка оригинального DXIL; см. overrides.rs);
//!   `--reload` — сразу перезагрузить их в запущенной игре (shim d3d12, TCP 47322).
//! `cargo xtask shader <хеш>.<стадия> [--ir]` — имя, сигнатуры, ресурсы шейдера из дампа; `--ir` — дизассемблер в `.local`.
//! `cargo xtask bench <журнал|метка> [<журнал2|метка2>] [--skip сек] [--game игра]` — отчёт по времени кадра.
//! `cargo xtask trace [--seconds N] [--label L] [--game игра]` — Metal System Trace запущенной игры + вердикт CPU/GPU.

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
        Some("shaders") => shaders(args.get(1).map_or("eldenring", String::as_str)),
        Some("overrides") => {
            let game = args.get(1).filter(|a| !a.starts_with("--")).map_or("eldenring", String::as_str);
            overrides::build(&project_dir()?, game)?;
            if flag("--reload") { reload_overrides() } else { Ok(()) }
        }
        Some("shader") => {
            let id = args.get(1).filter(|a| !a.starts_with("--")).context("использование: cargo xtask shader <хеш>.<стадия> [--ir] [--game игра]")?;
            let game = args.iter().position(|a| a == "--game").and_then(|i| args.get(i + 1)).map_or("eldenring", String::as_str);
            overrides::show(&project_dir()?, game, id, flag("--ir"))
        }
        Some("bench") => bench(&args[1..]),
        Some("trace") => trace(&args[1..]),
        _ => bail!("использование: cargo xtask bundle [--debug] | shim [--release] | oracle …"),
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask лежит в корне workspace").to_owned()
}

/// Общая папка проекта (`ProjectPort/`): рядом лежат репозитории игр и `.local/`.
fn project_dir() -> Result<PathBuf> {
    Ok(root().parent().context("корень ProjectPort")?.to_owned())
}

fn cargo(args: &[&str]) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo).args(args).current_dir(root()).status()?;
    if !status.success() {
        bail!("cargo {} завершился с ошибкой", args.join(" "));
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
    std::fs::copy(target.join(profile).join("grace-app"), macos.join("grace-app")).context("копирование бинарника")?;
    std::fs::write(app.join("Contents/Info.plist"), info_plist())?;

    // Ad-hoc подпись: без неё macOS на Apple Silicon не запустит собранный бандл.
    let status = Command::new("codesign").args(["--force", "--sign", "-"]).arg(&app).status()?;
    if !status.success() {
        bail!("codesign не прошёл");
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

/// Конвейер шейдеров по корпусу, снятому shim'ом d3d12 в `.local/shaders/<игра>/`.
/// DXIL → metallib: Metal Shader Converter (grace-irconv, под Rosetta).
/// DXBC SM4/5 → SPIR-V (vkd3d-compiler) → MSL (SPIRV-Cross) → metallib (xcrun metal).
/// Все metallib складываются в `.local/shaders/<игра>-metal/` и проверяются в Metal нативно.
fn shaders(game: &str) -> Result<()> {
    let local = root().parent().context("корень ProjectPort")?.join(".local");
    let corpus = local.join("shaders").join(game);
    let metal = local.join("shaders").join(format!("{game}-metal"));
    let work = local.join("shaders").join(format!("{game}-msl"));
    std::fs::create_dir_all(&metal)?;
    std::fs::create_dir_all(&work)?;
    let (corpus_s, metal_s) = (corpus.to_string_lossy().into_owned(), metal.to_string_lossy().into_owned());

    println!("== сводка корпуса");
    cargo(&["run", "-q", "--release", "-p", "grace-shader", "--", "inventory", &corpus_s])?;

    println!("== DXIL → metallib (Metal Shader Converter из GPTK, под Rosetta)");
    cargo(&["build", "-q", "--release", "-p", "grace-irconv", "--target", "x86_64-apple-darwin"])?;
    let status = Command::new(root().join("target/x86_64-apple-darwin/release/grace-irconv")).args([&corpus_s, &metal_s]).status()?;
    if !status.success() {
        bail!("grace-irconv завершился с ошибкой");
    }

    println!("== DXBC → SPIR-V → MSL → metallib (vkd3d + SPIRV-Cross + xcrun metal)");
    let vkd3d = local.join("tools/bin/vkd3d-compiler");
    if !vkd3d.exists() {
        bail!("нет {} — сборка vkd3d описана в docs/ARCHITECTURE.md (шейдерный конвейер)", vkd3d.display());
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
                eprintln!("{stem}: {tool}: {}", String::from_utf8_lossy(&res.stderr).lines().next().unwrap_or("ошибка"));
                good = false;
                break;
            }
        }
        if good { ok += 1 } else { failed += 1 }
    }
    println!("DXBC → metallib: успешно {ok}, ошибок {failed}");

    println!("== проверка в Metal (нативно)");
    cargo(&["run", "-q", "--release", "-p", "grace-shader", "--", "metallib", &metal_s])
}

/// Перезагрузка подмен в запущенной игре через канал управления shim d3d12.
fn reload_overrides() -> Result<()> {
    use std::io::BufReader;
    let addr = "127.0.0.1:47322";
    let stream = std::net::TcpStream::connect(addr).with_context(|| format!("нет соединения с {addr} — игра с shim запущена?"))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(60)))?;
    let mut w = stream.try_clone()?;
    grace_proto::write_message(&mut w, &grace_proto::Request::Overrides { op: grace_proto::OverrideOp::Reload })?;
    match grace_proto::read_message::<_, grace_proto::Response>(&mut BufReader::new(stream))? {
        Some(grace_proto::Response::Overrides(o)) => {
            println!("{}", o.last_reload);
            println!("подмены {} · пар PSO {}", if o.enabled { "ВКЛ" } else { "выкл" }, o.pairs);
            Ok(())
        }
        Some(grace_proto::Response::Error { message }) => bail!("shim: {message}"),
        other => bail!("неожиданный ответ shim: {other:?}"),
    }
}

/// Отчёт по журналам кадров. Аргумент — путь к CSV или метка прогона
/// (`.local/frametimes/<игра>/<метка>.csv`, игра по умолчанию eldenring).
fn bench(args: &[String]) -> Result<()> {
    let game = args.iter().position(|a| a == "--game").and_then(|i| args.get(i + 1)).map_or("eldenring", String::as_str);
    let dir = root().parent().context("корень ProjectPort")?.join(".local/frametimes").join(game);
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

/// Metal System Trace запущенной под эталоном игры и разбор: CPU- или GPU-bound.
fn trace(args: &[String]) -> Result<()> {
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let game = opt("--game").unwrap_or_else(|| "eldenring".into());
    let seconds = opt("--seconds").unwrap_or_else(|| "15".into());
    let label = opt("--label").unwrap_or_else(|| {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("trace-{secs}")
    });
    let project = root().parent().context("корень ProjectPort")?.to_owned();
    let cfg = grace_core::config::LocalConfig::discover(&root())?;
    let exe = cfg.games.get(&game).and_then(|g| g.exe.clone()).with_context(|| format!("в [games.{game}] нет exe"))?;

    // Процесс игры под Wine: wine64-preloader с именем exe в командной строке.
    let ps = Command::new("ps").args(["-axo", "pid=,command="]).output()?;
    let pid = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .find(|l| l.contains("wine64-preloader") && l.contains(exe.as_str()))
        .and_then(|l| l.split_whitespace().next().map(str::to_owned))
        .with_context(|| format!("{exe} не запущен — cargo xtask oracle run"))?;

    let dir = project.join(".local/traces").join(&game);
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{label}.trace"));
    if out.exists() {
        bail!("{} уже есть — задай другую --label", out.display());
    }
    println!("запись Metal System Trace: {exe} (pid {pid}), {seconds} с → {}", out.display());
    let status = Command::new("xcrun")
        .args(["xctrace", "record", "--template", "Metal System Trace", "--attach", &pid, "--time-limit", &format!("{seconds}s", seconds = seconds.trim_end_matches('s')), "--output"])
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        bail!("xctrace record завершился с ошибкой");
    }
    let out_s = out.to_string_lossy().into_owned();
    cargo(&["run", "-q", "--release", "-p", "grace-bench", "--", "trace", &out_s, "--pid", &pid])
}
