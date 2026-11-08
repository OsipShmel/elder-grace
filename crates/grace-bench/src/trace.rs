//! Metal System Trace (`xctrace`) analysis: GPU occupancy, per-thread CPU load, verdict.
//!
//! Tables are exported with `xcrun xctrace export --xpath …` as XML. Repeated values there are
//! replaced by `ref="N"` references to the element with `id="N"`, so the parser keeps an id table.
//! Column tags are engineering types (`start-time`, `duration`, `metal-nesting-level`, …).

use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

#[derive(Debug, Clone, Default)]
struct Info {
    text: String,
    fmt: String,
    pid: Option<u32>,
}

struct Frame {
    tag: String,
    id: Option<String>,
    info: Info,
}

fn attrs(e: &BytesStart<'_>) -> (Option<String>, Option<String>, String) {
    let (mut id, mut rf, mut fmt) = (None, None, String::new());
    for a in e.attributes().flatten() {
        let v = String::from_utf8_lossy(&a.value).into_owned();
        match a.key.as_ref() {
            b"id" => id = Some(v),
            b"ref" => rf = Some(v),
            b"fmt" => fmt = v,
            _ => {}
        }
    }
    (id, rf, fmt)
}

/// Calls `f` for every table row: column (tag) -> value (first occurrence of the tag).
fn for_each_row(path: &Path, mut f: impl FnMut(&HashMap<String, Info>)) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut reader = Reader::from_reader(BufReader::new(file));
    let mut buf = Vec::new();
    let mut ids: HashMap<String, Info> = HashMap::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut row: Option<(usize, HashMap<String, Info>)> = None;

    // Finished element: register its id, lift the pid to the parent, put the column into the row.
    let finish = |frame: Frame, stack: &mut Vec<Frame>, ids: &mut HashMap<String, Info>, row: &mut Option<(usize, HashMap<String, Info>)>| {
        let mut info = frame.info;
        if frame.tag == "pid" {
            info.pid = info.text.trim().parse().ok();
        }
        if let Some(id) = &frame.id {
            ids.insert(id.clone(), info.clone());
        }
        if let Some(parent) = stack.last_mut()
            && parent.info.pid.is_none()
        {
            parent.info.pid = info.pid;
        }
        if let Some((depth, cols)) = row.as_mut()
            && stack.len() == *depth + 1
        {
            cols.entry(frame.tag).or_insert(info);
        }
    };

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let (id, rf, fmt) = attrs(&e);
                let info = rf.and_then(|r| ids.get(&r).cloned()).unwrap_or(Info { fmt, ..Info::default() });
                if tag == "row" {
                    row = Some((stack.len(), HashMap::new()));
                }
                stack.push(Frame { tag, id, info });
            }
            Event::Empty(e) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let (id, rf, fmt) = attrs(&e);
                let info = rf.and_then(|r| ids.get(&r).cloned()).unwrap_or(Info { fmt, ..Info::default() });
                finish(Frame { tag, id, info }, &mut stack, &mut ids, &mut row);
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.info.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::End(_) => {
                let Some(frame) = stack.pop() else { continue };
                if frame.tag == "row" {
                    if let Some((_, cols)) = row.take() {
                        f(&cols);
                    }
                    continue;
                }
                finish(frame, &mut stack, &mut ids, &mut row);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

fn num(cols: &HashMap<String, Info>, tag: &str) -> Option<f64> {
    cols.get(tag).and_then(|i| i.text.trim().parse().ok())
}

#[derive(Debug, Clone)]
pub struct TraceSummary {
    pub span_ms: f64,
    pub gpu_busy_ms: f64,
    pub gpu_busy_pct: f64,
    pub gpu_frames: usize,
    pub cpu_total_pct: f64,
    /// Process threads: (name, average load in % of one core), descending.
    pub threads: Vec<(String, f64)>,
    pub verdict: String,
}

/// Union of intervals (ns) -> total length.
fn union_len(mut iv: Vec<(u64, u64)>) -> u64 {
    iv.sort_unstable();
    let (mut total, mut cur): (u64, Option<(u64, u64)>) = (0, None);
    for (s, e) in iv {
        cur = match cur {
            Some((cs, ce)) if s <= ce => Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce - cs;
                Some((s, e))
            }
            None => Some((s, e)),
        };
    }
    total + cur.map_or(0, |(s, e)| e - s)
}

fn export(trace: &Path, schema: &str, out: &Path) -> Result<()> {
    let xpath = format!("/trace-toc/run[@number=\"1\"]/data/table[@schema=\"{schema}\"]");
    let res = Command::new("xcrun").args(["xctrace", "export", "--input"]).arg(trace).args(["--xpath", &xpath]).output()?;
    if !res.status.success() {
        bail!("xctrace export {schema}: {}", String::from_utf8_lossy(&res.stderr).trim());
    }
    std::fs::write(out, res.stdout)?;
    Ok(())
}

/// Analyzes the trace for process `pid`. Intermediate XML goes into `work`.
pub fn analyze(trace: &Path, pid: u32, work: &Path) -> Result<TraceSummary> {
    std::fs::create_dir_all(work)?;
    let (gpu_xml, thr_xml) = (work.join("metal-gpu-intervals.xml"), work.join("thread-cpu.xml"));
    export(trace, "metal-gpu-intervals", &gpu_xml)?;
    export(trace, "ThreadCPUUsage", &thr_xml)?;

    // GPU: top-level intervals (nesting 0) of our process, merged.
    let mut iv = Vec::new();
    let mut frames = std::collections::HashSet::new();
    for_each_row(&gpu_xml, |cols| {
        if cols.get("process").and_then(|p| p.pid) != Some(pid) {
            return;
        }
        let depth = num(cols, "metal-nesting-level").unwrap_or(0.0);
        if let (Some(s), Some(d)) = (num(cols, "start-time"), num(cols, "duration"))
            && depth == 0.0
        {
            iv.push((s as u64, (s + d) as u64));
        }
        if let Some(f) = cols.get("gpu-frame-number").map(|i| i.text.trim().to_string())
            && !f.is_empty()
        {
            frames.insert(f);
        }
    })?;
    let span = match (iv.iter().map(|i| i.0).min(), iv.iter().map(|i| i.1).max()) {
        (Some(a), Some(b)) if b > a => b - a,
        _ => bail!("the trace has no GPU intervals for process {pid}"),
    };
    let busy = union_len(iv);

    // CPU: 10 ms samples, per-thread load in % of one core.
    let mut per: HashMap<String, f64> = HashMap::new();
    let (mut t0, mut t1) = (u64::MAX, 0u64);
    for_each_row(&thr_xml, |cols| {
        if cols.get("process").and_then(|p| p.pid) != Some(pid) {
            return;
        }
        let name = cols.get("thread").map(|t| t.fmt.split(" (").take(2).collect::<Vec<_>>().join(" (")).unwrap_or_default();
        if let (Some(s), Some(c)) = (num(cols, "start-time"), num(cols, "system-cpu-percent")) {
            t0 = t0.min(s as u64);
            t1 = t1.max(s as u64);
            *per.entry(name).or_default() += c;
        }
    })?;
    let buckets = if t1 > t0 { ((t1 - t0) / 10_000_000 + 1) as f64 } else { 1.0 };
    let mut threads: Vec<(String, f64)> = per.into_iter().map(|(n, v)| (n, v / buckets)).collect();
    threads.sort_by(|a, b| b.1.total_cmp(&a.1));
    let cpu_total: f64 = threads.iter().map(|t| t.1).sum();

    let gpu_pct = busy as f64 / span as f64 * 100.0;
    let top = threads.first().map_or(0.0, |t| t.1);
    let verdict = if gpu_pct >= 85.0 {
        format!("GPU-bound: the GPU is busy {gpu_pct:.0}% of the time")
    } else if top >= 85.0 {
        format!("CPU-bound: the busiest thread uses {top:.0}% of a core, the GPU is busy {gpu_pct:.0}%")
    } else {
        format!("neither the GPU ({gpu_pct:.0}%) nor any CPU thread (max {top:.0}%) is fully loaded: probably waiting/synchronization (vsync, Present, streaming)")
    };
    Ok(TraceSummary {
        span_ms: span as f64 / 1e6,
        gpu_busy_ms: busy as f64 / 1e6,
        gpu_busy_pct: gpu_pct,
        gpu_frames: frames.len(),
        cpu_total_pct: cpu_total,
        threads,
        verdict,
    })
}

pub fn format_summary(s: &TraceSummary) -> String {
    let mut out = format!(
        "trace {:.1} s: GPU frames {} (~{:.1} FPS)\n  GPU busy {:.0} ms = {:.1}%\n  process CPU: {:.0}% (100% = one core)",
        s.span_ms / 1000.0,
        s.gpu_frames,
        s.gpu_frames as f64 / (s.span_ms / 1000.0),
        s.gpu_busy_ms,
        s.gpu_busy_pct,
        s.cpu_total_pct
    );
    for (name, pct) in s.threads.iter().take(6) {
        out += &format!("\n    {pct:5.1}%  {name}");
    }
    out += &format!("\n  verdict: {}", s.verdict);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_union() {
        assert_eq!(union_len(vec![(0, 10), (5, 15), (20, 30)]), 25);
        assert_eq!(union_len(vec![(20, 30), (0, 10)]), 20);
        assert_eq!(union_len(vec![]), 0);
    }

    #[test]
    fn xml_refs_and_pid() {
        let dir = std::env::temp_dir().join("grace-bench-test");
        std::fs::create_dir_all(&dir).expect("tmp");
        let path = dir.join("t.xml");
        std::fs::write(
            &path,
            r#"<trace-query-result><node><row><start-time id="1" fmt="0">100</start-time><duration id="2" fmt="x">50</duration><process id="3" fmt="game (7)"><pid id="4" fmt="7">7</pid></process></row>
<row><start-time ref="1"/><duration ref="2"/><process ref="3"/></row></node></trace-query-result>"#,
        )
        .expect("write");
        let mut rows = Vec::new();
        for_each_row(&path, |c| rows.push((num(c, "start-time"), num(c, "duration"), c.get("process").and_then(|p| p.pid)))).expect("parse");
        assert_eq!(rows, vec![(Some(100.0), Some(50.0), Some(7)), (Some(100.0), Some(50.0), Some(7))]);
    }
}
