//! Разбор Metal System Trace (`xctrace`): занятость GPU, загрузка CPU по потокам, вердикт.
//!
//! Таблицы выгружаются `xcrun xctrace export --xpath …` в XML. В нём повторяющиеся значения
//! заменены ссылками `ref="N"` на элемент с `id="N"`, поэтому парсер хранит таблицу id.
//! Теги колонок — это engineering-type (`start-time`, `duration`, `metal-nesting-level`, …).

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

/// Вызывает `f` для каждой строки таблицы: колонка (тег) → значение (первое вхождение тега).
fn for_each_row(path: &Path, mut f: impl FnMut(&HashMap<String, Info>)) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("чтение {}", path.display()))?;
    let mut reader = Reader::from_reader(BufReader::new(file));
    let mut buf = Vec::new();
    let mut ids: HashMap<String, Info> = HashMap::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut row: Option<(usize, HashMap<String, Info>)> = None;

    // Завершённый элемент: регистрируем id, поднимаем pid к родителю, кладём колонку в строку.
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
    /// Потоки процесса: (имя, средняя загрузка в % одного ядра), по убыванию.
    pub threads: Vec<(String, f64)>,
    pub verdict: String,
}

/// Объединение интервалов (нс) → суммарная длина.
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

/// Разбирает трейс для процесса `pid`. Промежуточные XML кладутся в `work`.
pub fn analyze(trace: &Path, pid: u32, work: &Path) -> Result<TraceSummary> {
    std::fs::create_dir_all(work)?;
    let (gpu_xml, thr_xml) = (work.join("metal-gpu-intervals.xml"), work.join("thread-cpu.xml"));
    export(trace, "metal-gpu-intervals", &gpu_xml)?;
    export(trace, "ThreadCPUUsage", &thr_xml)?;

    // GPU: интервалы верхнего уровня (вложенность 0) нашего процесса, объединённые.
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
        _ => bail!("в трейсе нет GPU-интервалов процесса {pid}"),
    };
    let busy = union_len(iv);

    // CPU: сэмплы по 10 мс, загрузка в % одного ядра по потокам.
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
        format!("упирается в GPU: GPU занят {gpu_pct:.0}% времени")
    } else if top >= 85.0 {
        format!("упирается в CPU: самый загруженный поток — {top:.0}% ядра, GPU занят {gpu_pct:.0}%")
    } else {
        format!("ни GPU ({gpu_pct:.0}%), ни один поток CPU (макс. {top:.0}%) не загружены полностью — вероятно, ожидание/синхронизация (vsync, Present, подгрузка)")
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
        "трейс {:.1} с: кадров на GPU {} (~{:.1} FPS)\n  GPU занят {:.0} мс = {:.1}%\n  CPU процесса: {:.0}% (100% = одно ядро)",
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
    out += &format!("\n  вердикт: {}", s.verdict);
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
        .expect("запись");
        let mut rows = Vec::new();
        for_each_row(&path, |c| rows.push((num(c, "start-time"), num(c, "duration"), c.get("process").and_then(|p| p.pid)))).expect("разбор");
        assert_eq!(rows, vec![(Some(100.0), Some(50.0), Some(7)), (Some(100.0), Some(50.0), Some(7))]);
    }
}
