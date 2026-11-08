//! `grace-bench <log.csv> [<log2.csv>] [--skip <sec>]`
//! `grace-bench trace <file.trace> --pid <pid>`: Metal System Trace analysis.
//!
//! One file: a report. Two files: reports and a comparison of the second against the first.
//! `--skip` drops the first seconds (loading, warm-up; 5 s by default).

use std::path::PathBuf;

use anyhow::{Result, bail};
use grace_bench::{format_compare, format_report, load, report};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("trace") {
        let (Some(file), Some(pid)) = (args.get(1), args.iter().position(|a| a == "--pid").and_then(|i| args.get(i + 1))) else {
            bail!("usage: grace-bench trace <file.trace> --pid <pid>");
        };
        let trace = PathBuf::from(file);
        let work = trace.with_extension("export");
        let s = grace_bench::trace::analyze(&trace, pid.parse()?, &work)?;
        println!("{}", grace_bench::trace::format_summary(&s));
        return Ok(());
    }
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
        bail!("usage: grace-bench <log.csv> [<log2.csv>] [--skip <sec>]");
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
