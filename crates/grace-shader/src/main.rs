//! `grace-shader inventory <dir>`: summary of a shader corpus.
//! `grace-shader info <file.dxbc>`: one shader in detail.
//! `grace-shader names <dir> [substring] [--full]`: shader debug names (ILDN),
//!   to search by meaning, for example `names .local/shaders/<game> sky`.
//! `grace-shader metallib <dir>`: load every `.metallib` into Metal (natively) and
//! create its functions: checks that the shaders really work on the Apple GPU.

#[cfg(target_os = "macos")]
mod metal_check;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use grace_shader::{Bytecode, debug_name, inspect, inventory, short_name, signatures};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("inventory"), Some(dir)) => {
            let inv = inventory(&PathBuf::from(dir)).with_context(|| format!("reading {dir}"))?;
            println!("corpus {dir}: {} files, {} KB, unrecognized {}", inv.files, inv.bytes / 1024, inv.broken);
            println!("{:<6} {:<6} {:<7} {:>7} {:>12}", "format", "stage", "model", "files", "prog. size");
            for ((bc, stage, (maj, min)), (n, len)) in &inv.by_stage {
                let fmt = match bc {
                    Bytecode::Dxbc => "DXBC",
                    Bytecode::Dxil => "DXIL",
                };
                let unit = if *bc == Bytecode::Dxbc { "tok." } else { "B" };
                println!("{fmt:<6} {:<6} {:<7} {n:>7} {:>8} {unit}", stage.short(), format!("{maj}.{min}"), len);
            }
            if let Some((name, len)) = &inv.largest {
                println!("longest program: {name} ({len})");
            }
        }
        (Some("info"), Some(file)) => {
            let data = std::fs::read(file).with_context(|| format!("reading {file}"))?;
            match inspect(&data)? {
                Some(i) => println!(
                    "{:?} {} {}.{} · program {} · inputs {} · outputs {} · root signature: {} · chunks: {}",
                    i.bytecode,
                    i.stage.short(),
                    i.model.0,
                    i.model.1,
                    i.program_len,
                    i.inputs,
                    i.outputs,
                    if i.has_root_signature { "yes" } else { "no" },
                    i.chunks.join(" ")
                ),
                None => println!("the container has no shader program"),
            }
            let (inputs, outputs) = signatures(&data)?;
            for (title, sig) in [("inputs", inputs), ("outputs", outputs)] {
                for e in sig {
                    println!("  {title}: {}{} -> register {} mask {:#06b} (sv {}, type {})", e.semantic, e.index, e.register, e.mask, e.system_value, e.component_type);
                }
            }
        }
        (Some("names"), Some(dir)) => names(dir, args.get(2).filter(|a| !a.starts_with("--")), args.iter().any(|a| a == "--full"))?,
        #[cfg(target_os = "macos")]
        (Some("metallib"), Some(dir)) => metal_check::run(&PathBuf::from(dir))?,
        _ => bail!("usage: grace-shader inventory <dir> | info <file.dxbc> | names <dir> [substring] [--full] | metallib <dir>"),
    }
    Ok(())
}

/// Prints `<hash>.<stage>  <short name>` (or the full path) for the shaders in a directory,
/// filtered by a name substring (case-insensitive).
fn names(dir: &str, pattern: Option<&String>, full: bool) -> Result<()> {
    let pattern = pattern.map(|p| p.to_lowercase());
    let mut rows = Vec::new();
    let (mut total, mut named) = (0usize, 0usize);
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {dir}"))?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("dxbc") {
            continue;
        }
        total += 1;
        let Ok(data) = std::fs::read(&path) else { continue };
        let Some(debug) = debug_name(&data) else { continue };
        named += 1;
        if pattern.as_ref().is_some_and(|p| !debug.to_lowercase().contains(p.as_str())) {
            continue;
        }
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        rows.push((id, if full { debug.clone() } else { short_name(&debug).to_owned() }));
    }
    rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    for (id, name) in &rows {
        println!("{id:<22} {name}");
    }
    eprintln!("shaders {total}, named {named}, shown {}", rows.len());
    Ok(())
}
