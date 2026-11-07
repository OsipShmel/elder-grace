//! Подмены шейдеров: сборка из исходников игрового репозитория и справка по шейдеру.
//!
//! Источник подмены — файл `../grace-<игра>/shaders/overrides/<хеш>.<стадия>.<вид>`,
//! где хеш — fnv64 исходного шейдера из дампа (по нему shim находит, что подменять):
//! - `.hlsl` — полная замена. DXBC-оригинал → vkd3d (`<стадия>_5_0`), DXIL → dxc
//!   (модель оригинала, например `ps_6_0`).
//! - `.irpatch` — правка **оригинального** DXIL без его исходников: shim-независимые
//!   операции над выходами (см. [`apply_irpatch`]). Дизассемблер dxc → правка IR →
//!   ассемблер dxa → валидатор dxv. Код игры в репозиторий не попадает: в git лежит
//!   только патч, промежуточный `.ll` — в `.local/shaders/<игра>-override-src/`.
//! - `.local/shaders/<игра>-override-ll/<хеш>.<стадия>.ll` — свободная правка IR руками,
//!   только локально (это дизассемблированный код игры, в git нельзя).
//!
//! Результат — `.local/shaders/<игра>-override/<хеш>.<стадия>.dxbc`; лишние файлы
//! оттуда удаляются, чтобы удалённая подмена не продолжала действовать.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use grace_shader::{Bytecode, SignatureElement, debug_name, inspect, short_name, signatures};

struct Tools {
    vkd3d: PathBuf,
    dxc: PathBuf,
    dxa: PathBuf,
    dxv: PathBuf,
}

impl Tools {
    fn find(project: &Path) -> Self {
        let bin = project.join(".local/tools/bin");
        Self { vkd3d: bin.join("vkd3d-compiler"), dxc: bin.join("dxc"), dxa: bin.join("dxa"), dxv: bin.join("dxv") }
    }

    fn need(path: &Path, hint: &str) -> Result<()> {
        if !path.exists() {
            bail!("нет {} — {hint}", path.display());
        }
        Ok(())
    }
}

struct Dirs {
    src: PathBuf,
    local_ll: PathBuf,
    generated: PathBuf,
    dump: PathBuf,
    out: PathBuf,
}

impl Dirs {
    fn new(project: &Path, game: &str) -> Self {
        let shaders = project.join(".local/shaders");
        Self {
            src: project.join(format!("grace-{game}/shaders/overrides")),
            local_ll: shaders.join(format!("{game}-override-ll")),
            generated: shaders.join(format!("{game}-override-src")),
            dump: shaders.join(game),
            out: shaders.join(format!("{game}-override")),
        }
    }
}

fn run(cmd: &mut Command) -> Result<std::process::Output> {
    let out = cmd.output().with_context(|| format!("запуск {:?}", cmd.get_program()))?;
    if !out.status.success() {
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        bail!("{}", text.trim());
    }
    Ok(out)
}

/// `cargo xtask overrides [игра]`.
pub fn build(project: &Path, game: &str) -> Result<()> {
    let dirs = Dirs::new(project, game);
    let tools = Tools::find(project);
    std::fs::create_dir_all(&dirs.out)?;
    std::fs::create_dir_all(&dirs.generated)?;

    let mut sources: Vec<PathBuf> = Vec::new();
    for dir in [&dirs.src, &dirs.local_ll] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            sources.extend(rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| matches!(ext(p), "hlsl" | "irpatch" | "ll")));
        }
    }
    sources.sort();

    let (mut ok, mut failed, mut built) = (0, 0, BTreeSet::new());
    for path in &sources {
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut parts = file.split('.');
        let (Some(hash), Some(stage)) = (parts.next(), parts.next()) else { continue };
        let id = format!("{hash}.{stage}");
        if !built.insert(id.clone()) {
            println!("  {file}: ПРОПУЩЕН — для {id} уже есть другой источник");
            failed += 1;
            continue;
        }
        match build_one(&dirs, &tools, path, &id, stage) {
            Ok(warnings) => {
                println!("  {file}: собран");
                for w in warnings {
                    println!("    внимание: {w}");
                }
                ok += 1;
            }
            Err(e) => {
                println!("  {file}: ОШИБКА {e:#}");
                built.remove(&id);
                failed += 1;
            }
        }
    }

    // Подмена, источник которой удалён, не должна продолжать действовать.
    // Если источник есть, но не собрался, остаётся последняя удачная сборка.
    let mut removed = 0;
    for entry in std::fs::read_dir(&dirs.out)?.flatten() {
        let p = entry.path();
        let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if ext(&p) == "dxbc" && !built.contains(&id) && !sources.iter().any(|s| s.to_string_lossy().contains(&id)) {
            std::fs::remove_file(&p)?;
            removed += 1;
        }
    }
    println!("подмен собрано {ok}, ошибок {failed}, убрано устаревших {removed} → {}", dirs.out.display());
    if failed > 0 {
        bail!("не все подмены собрались");
    }
    Ok(())
}

fn ext(p: &Path) -> &str {
    p.extension().and_then(|e| e.to_str()).unwrap_or("")
}

/// Собирает одну подмену; возвращает предупреждения сверки сигнатур.
fn build_one(dirs: &Dirs, tools: &Tools, path: &Path, id: &str, stage: &str) -> Result<Vec<String>> {
    let original_path = dirs.dump.join(format!("{id}.dxbc"));
    let original = std::fs::read(&original_path).with_context(|| format!("нет оригинала {} в дампе — шейдер ещё не попадался игре?", original_path.display()))?;
    let info = inspect(&original)?.context("в оригинале нет программы шейдера")?;
    let target = dirs.out.join(format!("{id}.dxbc"));
    match (ext(path), info.bytecode) {
        ("hlsl", Bytecode::Dxbc) => {
            Tools::need(&tools.vkd3d, "см. сборку vkd3d в docs/ARCHITECTURE.md")?;
            run(Command::new(&tools.vkd3d)
                .args(["-x", "hlsl", "-p", &format!("{stage}_5_0"), "-b", "dxbc-tpf", "-e", "main", "-o"])
                .arg(&target)
                .arg(path))?;
        }
        ("hlsl", Bytecode::Dxil) => {
            Tools::need(&tools.dxc, "см. сборку dxc в docs/ARCHITECTURE.md")?;
            let profile = format!("{stage}_{}_{}", info.model.0, info.model.1);
            run(Command::new(&tools.dxc).args(["-T", &profile, "-E", "main", "-Fo"]).arg(&target).arg(path))?;
        }
        ("irpatch", Bytecode::Dxil) => {
            Tools::need(&tools.dxc, "см. сборку dxc в docs/ARCHITECTURE.md")?;
            let ir = String::from_utf8(run(Command::new(&tools.dxc).arg("-dumpbin").arg(&original_path))?.stdout)?;
            let patch = std::fs::read_to_string(path)?;
            let (patched, changed) = apply_irpatch(&ir, &patch)?;
            if changed == 0 {
                bail!("патч ничего не изменил — проверь номера выходов (cargo xtask shader {id})");
            }
            let ll = dirs.generated.join(format!("{id}.ll"));
            std::fs::write(&ll, patched)?;
            assemble(tools, &ll, &target)?;
        }
        ("ll", Bytecode::Dxil) => assemble(tools, path, &target)?,
        (kind, Bytecode::Dxbc) if kind != "hlsl" => bail!("`.{kind}` — только для DXIL, а оригинал DXBC (пиши .hlsl)"),
        (kind, _) => bail!("неизвестный вид источника `.{kind}`"),
    }
    let built = std::fs::read(&target)?;
    Ok(compare_signatures(&original, &built))
}

fn assemble(tools: &Tools, ll: &Path, target: &Path) -> Result<()> {
    Tools::need(&tools.dxa, "см. сборку dxc в docs/ARCHITECTURE.md (цели dxa, dxv)")?;
    run(Command::new(&tools.dxa).arg(ll).arg("-o").arg(target)).context("dxa")?;
    run(Command::new(&tools.dxv).arg(target)).context("dxv: валидация DXIL")?;
    Ok(())
}

fn sig_key(e: &SignatureElement) -> (String, u32, u32, u32, u32, u8) {
    (e.semantic.to_uppercase(), e.index, e.register, e.system_value, e.component_type, e.mask)
}

/// Расхождения сигнатур входов/выходов подмены и оригинала: D3DMetal связывает стадии
/// по ним, несовпадение — частая причина мусора на экране.
fn compare_signatures(original: &[u8], built: &[u8]) -> Vec<String> {
    let (Ok((oi, oo)), Ok((bi, bo))) = (signatures(original), signatures(built)) else {
        return vec!["не удалось разобрать сигнатуры".into()];
    };
    let mut warnings = Vec::new();
    for (title, a, b) in [("входы", oi, bi), ("выходы", oo, bo)] {
        let (ka, kb): (Vec<_>, Vec<_>) = (a.iter().map(sig_key).collect(), b.iter().map(sig_key).collect());
        if ka != kb {
            let fmt = |v: &[SignatureElement]| v.iter().map(|e| format!("{}{}@r{}", e.semantic, e.index, e.register)).collect::<Vec<_>>().join(" ");
            warnings.push(format!("{title} не совпадают с оригиналом: было [{}], стало [{}]", fmt(&a), fmt(&b)));
        }
    }
    warnings
}

/// Применяет `.irpatch` к тексту DXIL (вывод `dxc -dumpbin`). Строки патча:
///
/// ```text
/// # комментарий
/// output <id> mul <x> [<y> <z> <w>]   # умножить компоненты выхода <id>
/// output <id> add <x> [<y> <z> <w>]   # прибавить
/// ```
///
/// `<id>` — номер элемента выходной сигнатуры (у пиксельного шейдера с одним
/// `SV_Target0` это 0; список — `cargo xtask shader <хеш>.<стадия>`). Правится каждый
/// `dx.op.storeOutput.f32` этого выхода: значение заменяется на `fmul`/`fadd` с константой.
/// Возвращает новый текст и число изменённых записей.
pub fn apply_irpatch(ir: &str, patch: &str) -> Result<(String, usize)> {
    let mut text = ir.to_owned();
    let mut total = 0;
    for (n, line) in patch.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let (op, output, values) = match words.as_slice() {
            ["output", id, op @ ("mul" | "add"), vals @ ..] if !vals.is_empty() && vals.len() <= 4 => {
                let id: u32 = id.parse().with_context(|| format!("строка {}: номер выхода", n + 1))?;
                let vals: Vec<f32> = vals.iter().map(|v| v.parse::<f32>()).collect::<Result<_, _>>().with_context(|| format!("строка {}: числа", n + 1))?;
                (if *op == "mul" { "fmul" } else { "fadd" }, id, vals)
            }
            _ => bail!("строка {}: не понимаю `{line}` (ожидается `output <id> mul|add <x> [y z w]`)", n + 1),
        };
        let (next, changed) = patch_outputs(&text, n, op, output, &values);
        text = next;
        total += changed;
    }
    Ok((text, total))
}

/// Константа float в виде, который LLVM IR принимает без потери точности.
fn ir_float(v: f32) -> String {
    format!("0x{:016X}", f64::from(v).to_bits())
}

fn patch_outputs(ir: &str, pass: usize, op: &str, output: u32, values: &[f32]) -> (String, usize) {
    const CALL: &str = "call void @dx.op.storeOutput.f32(";
    let mut out = String::with_capacity(ir.len() + 256);
    let mut changed = 0;
    for line in ir.lines() {
        let rewritten = (|| {
            let start = line.find(CALL)? + CALL.len();
            let end = start + line[start..].find(')')?;
            let args: Vec<&str> = line[start..end].split(", ").collect();
            let [opcode, id, row, col, value] = args.as_slice() else { return None };
            if id.strip_prefix("i32 ")?.parse::<u32>().ok()? != output {
                return None;
            }
            let col: usize = col.strip_prefix("i8 ")?.parse().ok()?;
            let constant = *values.get(col)?;
            let value = value.strip_prefix("float ")?;
            let indent = &line[..line.len() - line.trim_start().len()];
            let name = format!("%grace_{pass}_{changed}");
            Some(format!(
                "{indent}{name} = {op} fast float {value}, {}\n{}{opcode}, {id}, {row}, i8 {col}, float {name}{}",
                ir_float(constant),
                &line[..start],
                &line[end..]
            ))
        })();
        match rewritten {
            Some(new) => {
                out.push_str(&new);
                changed += 1;
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    (out, changed)
}

/// `cargo xtask shader <хеш>.<стадия> [--ir]` — что это за шейдер и как его подменять.
pub fn show(project: &Path, game: &str, id: &str, write_ir: bool) -> Result<()> {
    let dirs = Dirs::new(project, game);
    let tools = Tools::find(project);
    let id = id.trim_end_matches(".dxbc");
    let path = dirs.dump.join(format!("{id}.dxbc"));
    let data = std::fs::read(&path).with_context(|| format!("нет {} в дампе", path.display()))?;
    let info = inspect(&data)?.context("в контейнере нет программы шейдера")?;
    println!("{id}: {:?} {} {}.{}", info.bytecode, info.stage.short(), info.model.0, info.model.1);
    if let Some(name) = debug_name(&data) {
        println!("имя: {} ({name})", short_name(&name));
    }
    let (inputs, outputs) = signatures(&data)?;
    for (title, sig) in [("вход", inputs), ("выход", outputs)] {
        for (i, e) in sig.iter().enumerate() {
            println!("  {title} {i}: {}{} · регистр {} · маска {:#06b}", e.semantic, e.index, e.register, e.mask);
        }
    }
    if info.bytecode == Bytecode::Dxil {
        Tools::need(&tools.dxc, "см. сборку dxc в docs/ARCHITECTURE.md")?;
        let ir = String::from_utf8(run(Command::new(&tools.dxc).arg("-dumpbin").arg(&path))?.stdout)?;
        // Секция «; Resource Bindings:», пустая строка «;», таблица до следующей «;».
        let bindings: Vec<&str> = ir.lines().skip_while(|l| !l.contains("Resource Bindings:")).skip(2).take_while(|l| l.trim() != ";").collect();
        if !bindings.is_empty() {
            println!("ресурсы:");
            for l in bindings {
                println!("  {}", l.trim_start_matches(';'));
            }
        }
        if write_ir {
            std::fs::create_dir_all(&dirs.generated)?;
            let ll = dirs.generated.join(format!("{id}.orig.ll"));
            std::fs::write(&ll, ir)?;
            println!("дизассемблер: {} (код игры — только локально; правленую копию клади в {})", ll.display(), dirs.local_ll.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IR: &str = "\
define void @main() {
  %1 = fadd fast float %0, 1.0
  call void @dx.op.storeOutput.f32(i32 5, i32 0, i32 0, i8 0, float %1)  ; StoreOutput(outputSigId,rowIndex,colIndex,value)
  call void @dx.op.storeOutput.f32(i32 5, i32 0, i32 0, i8 3, float 1.000000e+00)  ; StoreOutput
  call void @dx.op.storeOutput.f32(i32 5, i32 1, i32 0, i8 0, float %1)  ; StoreOutput
  ret void
}
declare void @dx.op.storeOutput.f32(i32, i32, i32, i8, float) #1
";

    #[test]
    fn irpatch_multiplies_only_requested_output_and_channels() {
        let (out, n) = apply_irpatch(IR, "# оттенок\noutput 0 mul 0.5 1 1\n").unwrap();
        assert_eq!(n, 1, "канал 3 без значения и выход 1 не трогаем");
        assert!(out.contains("  %grace_1_0 = fmul fast float %1, 0x3FE0000000000000\n  call void @dx.op.storeOutput.f32(i32 5, i32 0, i32 0, i8 0, float %grace_1_0)  ; StoreOutput("));
        assert!(out.contains("i8 3, float 1.000000e+00)"));
        assert!(out.contains("i32 1, i32 0, i8 0, float %1)"));
        assert!(out.contains("declare void @dx.op.storeOutput.f32(i32, i32, i32, i8, float) #1"));
    }

    #[test]
    fn irpatch_chains_and_rejects_garbage() {
        let (out, n) = apply_irpatch(IR, "output 0 mul 2 2 2 2\noutput 0 add 0.25 0 0 0\n").unwrap();
        assert_eq!(n, 4);
        assert!(out.contains("%grace_1_0 = fadd fast float %grace_0_0, 0x3FD0000000000000"));
        assert!(apply_irpatch(IR, "output x mul 1").is_err());
        assert!(apply_irpatch(IR, "tint red").is_err());
    }
}
