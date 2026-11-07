//! `grace-bench <журнал.csv> [<журнал2.csv>] [--skip <сек>]`
//!
//! Один файл — отчёт. Два файла — отчёты и сравнение второго с первым.
//! `--skip` отбрасывает первые секунды (загрузка, прогрев; по умолчанию 5 с).

use std::path::PathBuf;

use anyhow::{Result, bail};
use grace_bench::{format_compare, format_report, load, report};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut skip_s = 5.0;
    let mut files = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--skip" {
            skip_s = it.next().map(|v| v.parse()).transpose()?.unwrap_or(skip_s);
        } else {
            files.push(PathBuf::from(a));
        }
    }
    if files.is_empty() || files.len() > 2 {
        bail!("использование: grace-bench <журнал.csv> [<журнал2.csv>] [--skip <сек>]");
    }
    let name = |p: &PathBuf| p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut reports = Vec::new();
    for f in &files {
        let r = report(&load(f)?, skip_s * 1000.0)?;
        println!("{}\n", format_report(&name(f), &r));
        reports.push(r);
    }
    if let [a, b] = reports.as_slice() {
        println!("{}", format_compare(&name(&files[0]), a, &name(&files[1]), b));
    }
    Ok(())
}
