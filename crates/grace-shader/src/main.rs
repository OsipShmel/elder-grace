//! `grace-shader inventory <каталог>` — сводка по корпусу шейдеров.
//! `grace-shader info <файл.dxbc>` — разбор одного шейдера.
//! `grace-shader metallib <каталог>` — загрузить каждый `.metallib` в Metal (нативно) и
//! создать его функции: проверка, что шейдеры реально годятся для Apple GPU.

#[cfg(target_os = "macos")]
mod metal_check;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use grace_shader::{Bytecode, inspect, inventory, signatures};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("inventory"), Some(dir)) => {
            let inv = inventory(&PathBuf::from(dir)).with_context(|| format!("чтение {dir}"))?;
            println!("корпус {dir}: {} файлов, {} КБ, нераспознано {}", inv.files, inv.bytes / 1024, inv.broken);
            println!("{:<6} {:<6} {:<7} {:>7} {:>12}", "формат", "стадия", "модель", "файлов", "размер прог.");
            for ((bc, stage, (maj, min)), (n, len)) in &inv.by_stage {
                let fmt = match bc {
                    Bytecode::Dxbc => "DXBC",
                    Bytecode::Dxil => "DXIL",
                };
                let unit = if *bc == Bytecode::Dxbc { "ток." } else { "Б" };
                println!("{fmt:<6} {:<6} {:<7} {n:>7} {:>8} {unit}", stage.short(), format!("{maj}.{min}"), len);
            }
            if let Some((name, len)) = &inv.largest {
                println!("самая длинная программа: {name} ({len})");
            }
        }
        (Some("info"), Some(file)) => {
            let data = std::fs::read(file).with_context(|| format!("чтение {file}"))?;
            match inspect(&data)? {
                Some(i) => println!(
                    "{:?} {} {}.{} · программа {} · входов {} · выходов {} · корневая сигнатура: {} · чанки: {}",
                    i.bytecode,
                    i.stage.short(),
                    i.model.0,
                    i.model.1,
                    i.program_len,
                    i.inputs,
                    i.outputs,
                    if i.has_root_signature { "да" } else { "нет" },
                    i.chunks.join(" ")
                ),
                None => println!("в контейнере нет программы шейдера"),
            }
            let (inputs, outputs) = signatures(&data)?;
            for (title, sig) in [("входы", inputs), ("выходы", outputs)] {
                for e in sig {
                    println!("  {title}: {}{} → регистр {} маска {:#06b} (sv {}, тип {})", e.semantic, e.index, e.register, e.mask, e.system_value, e.component_type);
                }
            }
        }
        #[cfg(target_os = "macos")]
        (Some("metallib"), Some(dir)) => metal_check::run(&PathBuf::from(dir))?,
        _ => bail!("использование: grace-shader inventory <каталог> | info <файл.dxbc> | metallib <каталог>"),
    }
    Ok(())
}
