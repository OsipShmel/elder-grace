//! Frame-time reports.
//!
//! Input is the `frame,t_ms,dt_ms` CSV from the `dxgi.dll` shim (one line per `Present`).
//! Metrics:
//! - average FPS = frames / duration (not the mean of instantaneous FPS);
//! - frame-time percentiles (p50/p95/p99);
//! - "1% low" / "0.1% low": FPS from the mean time of the worst 1% / 0.1% of frames;
//! - stutters: frames longer than `max(2 x median, 25 ms)`.

use std::path::Path;

pub mod trace;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Frame {
    pub t_ms: f64,
    pub dt_ms: f64,
}

pub fn parse_csv(text: &str) -> Result<Vec<Frame>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 && line.starts_with("frame") {
            continue;
        }
        let mut it = line.split(',');
        let (Some(_), Some(t), Some(dt)) = (it.next(), it.next(), it.next()) else { continue };
        out.push(Frame {
            t_ms: t.trim().parse().with_context(|| format!("line {}: t_ms", i + 1))?,
            dt_ms: dt.trim().parse().with_context(|| format!("line {}: dt_ms", i + 1))?,
        });
    }
    Ok(out)
}

pub fn load(path: &Path) -> Result<Vec<Frame>> {
    parse_csv(&std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)
}

#[derive(Debug, Clone)]
pub struct Report {
    pub frames: usize,
    pub seconds: f64,
    pub avg_fps: f64,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub low1_fps: f64,
    pub low01_fps: f64,
    pub stutters: usize,
    pub stutter_threshold_ms: f64,
    /// Frame-time histogram: (bucket upper bound in ms, frame count).
    pub histogram: Vec<(f64, usize)>,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// FPS from the mean time of the worst `share` of frames.
fn low_fps(sorted: &[f64], share: f64) -> f64 {
    let n = ((sorted.len() as f64 * share).ceil() as usize).max(1).min(sorted.len());
    let worst = &sorted[sorted.len() - n..];
    let avg = worst.iter().sum::<f64>() / worst.len() as f64;
    if avg > 0.0 { 1000.0 / avg } else { 0.0 }
}

const BUCKETS: [f64; 8] = [8.33, 11.11, 16.67, 20.0, 25.0, 33.33, 50.0, f64::INFINITY];

/// Report over frames starting `skip_ms` after the first frame (warm-up, loading).
pub fn report(frames: &[Frame], skip_ms: f64) -> Result<Report> {
    let dts: Vec<f64> = frames.iter().filter(|f| f.t_ms >= skip_ms && f.dt_ms > 0.0).map(|f| f.dt_ms).collect();
    if dts.len() < 2 {
        bail!("too few frames for a report ({})", dts.len());
    }
    let total_ms: f64 = dts.iter().sum();
    let mut sorted = dts.clone();
    sorted.sort_by(f64::total_cmp);
    let p50 = percentile(&sorted, 0.50);
    let threshold = (p50 * 2.0).max(25.0);
    let mut histogram: Vec<(f64, usize)> = BUCKETS.iter().map(|b| (*b, 0)).collect();
    for dt in &dts {
        if let Some(b) = histogram.iter_mut().find(|(edge, _)| dt <= edge) {
            b.1 += 1;
        }
    }
    Ok(Report {
        frames: dts.len(),
        seconds: total_ms / 1000.0,
        avg_fps: dts.len() as f64 / (total_ms / 1000.0),
        avg_ms: total_ms / dts.len() as f64,
        p50_ms: p50,
        p95_ms: percentile(&sorted, 0.95),
        p99_ms: percentile(&sorted, 0.99),
        max_ms: *sorted.last().unwrap_or(&0.0),
        low1_fps: low_fps(&sorted, 0.01),
        low01_fps: low_fps(&sorted, 0.001),
        stutters: dts.iter().filter(|d| **d > threshold).count(),
        stutter_threshold_ms: threshold,
        histogram,
    })
}

pub fn format_report(name: &str, r: &Report) -> String {
    let mut s = format!(
        "{name}: {} frames in {:.1} s\n  average FPS {:.1} ({:.2} ms) · 1% low {:.1} · 0.1% low {:.1}\n  frame time: p50 {:.2} · p95 {:.2} · p99 {:.2} · max {:.1} ms\n  stutters (> {:.1} ms): {}\n  histogram:",
        r.frames, r.seconds, r.avg_fps, r.avg_ms, r.low1_fps, r.low01_fps, r.p50_ms, r.p95_ms, r.p99_ms, r.max_ms, r.stutter_threshold_ms, r.stutters
    );
    let mut lower = 0.0;
    for (edge, n) in &r.histogram {
        let label = if edge.is_infinite() { format!("> {lower:.0} ms") } else { format!("{lower:.0}–{edge:.1} ms") };
        let bar = "█".repeat((n * 40).div_ceil(r.frames.max(1)).min(40));
        s += &format!("\n    {label:<14} {n:>6} {bar}");
        lower = *edge;
    }
    s
}

/// Comparison "B relative to A": a positive percentage means B is better.
pub fn format_compare(a_name: &str, a: &Report, b_name: &str, b: &Report) -> String {
    let better_up = |x: f64, y: f64| if x > 0.0 { (y - x) / x * 100.0 } else { 0.0 };
    let better_down = |x: f64, y: f64| if x > 0.0 { (x - y) / x * 100.0 } else { 0.0 };
    format!(
        "{b_name} vs {a_name}:\n  average FPS  {:>7.1} → {:>7.1}  ({:+.1}%)\n  1% low       {:>7.1} → {:>7.1}  ({:+.1}%)\n  p95, ms      {:>7.2} → {:>7.2}  ({:+.1}%)\n  stutters     {:>7} → {:>7}",
        a.avg_fps,
        b.avg_fps,
        better_up(a.avg_fps, b.avg_fps),
        a.low1_fps,
        b.low1_fps,
        better_up(a.low1_fps, b.low1_fps),
        a.p95_ms,
        b.p95_ms,
        better_down(a.p95_ms, b.p95_ms),
        a.stutters,
        b.stutters,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(dts: &[f64]) -> Vec<Frame> {
        let mut t = 0.0;
        dts.iter()
            .map(|dt| {
                t += dt;
                Frame { t_ms: t, dt_ms: *dt }
            })
            .collect()
    }

    #[test]
    fn steady_60fps() {
        let r = report(&synth(&[16.6667; 600]), 0.0).expect("report");
        assert!((r.avg_fps - 60.0).abs() < 0.01, "{}", r.avg_fps);
        assert!((r.low1_fps - 60.0).abs() < 0.01);
        assert_eq!(r.stutters, 0);
    }

    #[test]
    fn stutters_and_lows() {
        // 990 frames of 20 ms and 10 frames of 100 ms.
        let mut dts = vec![20.0; 990];
        dts.extend(vec![100.0; 10]);
        let r = report(&synth(&dts), 0.0).expect("report");
        assert_eq!(r.stutters, 10);
        assert!((r.low1_fps - 10.0).abs() < 0.01, "1% low = worst 10 frames of 100 ms -> 10 FPS");
        assert!((r.avg_fps - 1000.0 / 20.8).abs() < 0.01);
    }

    #[test]
    fn skip_warmup_and_parse() {
        let csv = "frame,t_ms,dt_ms\n1,0.000,0.000\n2,500.0,500.0\n3,520.0,20.0\n4,540.0,20.0\n5,560.0,20.0\n";
        let frames = parse_csv(csv).expect("csv");
        assert_eq!(frames.len(), 5);
        let r = report(&frames, 510.0).expect("report");
        assert_eq!(r.frames, 3);
        assert!((r.avg_fps - 50.0).abs() < 0.01);
    }
}
