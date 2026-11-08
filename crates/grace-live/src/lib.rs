//! Host side of the live mirror (ADR 0006): talk to the in-game shims, read the mirror
//! file, keep a live session going. What the mirrored bytes mean is the game's business:
//! a game passes a `discover` function (which regions to mirror) and a `decode` function.

use std::collections::VecDeque;
use std::io::BufReader;
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use grace_proto::mirror::{self, Snapshot};
use grace_proto::{MirrorRegion, Request, Response, read_message, write_message};

/// Control port of the dxgi shim (frame stats, live mirror).
pub fn dxgi_port() -> u16 {
    std::env::var("GRACE_SHIM_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(47321)
}

/// Control port of the d3d12 shim (memory search, overrides).
pub fn d3d12_port() -> u16 {
    std::env::var("GRACE_D3D12_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(47322)
}

/// A blocking client of a shim control port (`grace-proto`, one JSON line per message).
pub struct ShimClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl ShimClient {
    pub fn connect(port: u16) -> Result<Self> {
        let stream = TcpStream::connect(("127.0.0.1", port)).with_context(|| format!("shim on 127.0.0.1:{port}: is the game running under the oracle?"))?;
        // Memory walks take tens of seconds.
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        Ok(Self { writer: stream.try_clone()?, reader: BufReader::new(stream) })
    }

    pub fn ask(&mut self, req: &Request) -> Result<Response> {
        write_message(&mut self.writer, req)?;
        match read_message(&mut self.reader)? {
            Some(Response::Error { message }) => bail!("shim: {message}"),
            Some(r) => Ok(r),
            None => bail!("shim closed the connection"),
        }
    }
}

/// The mirror file mapped read-only.
pub struct MirrorReader {
    map: memmap2::Mmap,
}

impl MirrorReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).with_context(|| format!("mirror file {}", path.display()))?;
        if file.metadata()?.len() < mirror::SIZE as u64 {
            bail!("{} is not a mirror record yet (the shim creates it at the first frame)", path.display());
        }
        // SAFETY: the shim only writes inside the record; reads go through the seqlock.
        let map = unsafe { memmap2::MmapOptions::new().len(mirror::SIZE).map(&file)? };
        Ok(Self { map })
    }

    pub fn seq(&self) -> u64 {
        // SAFETY: the map is page-aligned and holds mirror::SIZE bytes.
        unsafe { mirror::current_seq(self.map.as_ptr()) }
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        // SAFETY: as above.
        unsafe { mirror::read(self.map.as_ptr()) }
    }
}

/// Session state reported to the caller every frame.
#[derive(Debug, Clone)]
pub enum LiveStatus<T> {
    /// Looking for what to mirror (memory search in progress or retrying).
    Searching(String),
    /// A state decoded from the latest game frame.
    Fresh(T),
    /// The game has not produced a frame for a while (paused, minimized); the last state.
    Stale(T),
}

const STALE: Duration = Duration::from_millis(250);
const REDISCOVER: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_secs(2);

type Discover = Arc<dyn Fn() -> Result<Vec<MirrorRegion>> + Send + Sync>;
type Decode<T> = Box<dyn Fn(&Snapshot) -> Option<T>>;

/// Arrival times of new mirror frames, stamped by a 1 kHz watcher with the host clock
/// (so sample age needs no mapping between Wine's QPC and the host clock).
#[derive(Default)]
struct Arrivals {
    frames: VecDeque<(u64, Instant)>,
}

/// A live session: discovery on a background thread, mirroring, decoding, rediscovery
/// when the mirrored bytes stop decoding, and stats.
pub struct LiveSession<T> {
    reader: Arc<MirrorReader>,
    arrivals: Arc<Mutex<Arrivals>>,
    discover: Discover,
    decode: Decode<T>,
    search: Option<JoinHandle<Result<Vec<MirrorRegion>>>>,
    retry_at: Option<Instant>,
    note: String,
    generation: Option<u32>,
    last: Option<T>,
    last_good: Option<Instant>,
    ages: VecDeque<f64>,
    copy_ns: u32,
}

impl<T: Clone> LiveSession<T> {
    pub fn new(mirror_file: &Path, discover: impl Fn() -> Result<Vec<MirrorRegion>> + Send + Sync + 'static, decode: impl Fn(&Snapshot) -> Option<T> + 'static) -> Result<Self> {
        let reader = Arc::new(MirrorReader::open(mirror_file)?);
        let arrivals = Arc::new(Mutex::new(Arrivals::default()));
        let (r, a) = (Arc::clone(&reader), Arc::clone(&arrivals));
        std::thread::Builder::new().name("grace-live-watch".into()).spawn(move || {
            let mut last = 0;
            loop {
                let s = r.seq();
                if s != last && s % 2 == 0 {
                    last = s;
                    if let Ok(mut a) = a.lock() {
                        a.frames.push_back((s, Instant::now()));
                        if a.frames.len() > 512 {
                            a.frames.pop_front();
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })?;
        Ok(Self {
            reader,
            arrivals,
            discover: Arc::new(discover),
            decode: Box::new(decode),
            search: None,
            retry_at: None,
            note: "starting".into(),
            generation: None,
            last: None,
            last_good: None,
            ages: VecDeque::new(),
            copy_ns: 0,
        })
    }

    /// Call once per rendered frame.
    pub fn poll(&mut self) -> LiveStatus<T> {
        if self.generation.is_none() {
            return self.poll_search();
        }
        let Some(snap) = self.reader.snapshot() else { return self.searching("mirror record unreadable") };
        if Some(snap.info.generation) != self.generation {
            // The shim has not switched to our region set yet.
            return self.last.clone().map_or_else(|| self.searching("waiting for the first mirrored frame"), LiveStatus::Stale);
        }
        self.copy_ns = snap.info.copy_ns;
        let arrived = self.arrived(snap.seq);
        match (self.decode)(&snap) {
            Some(state) => {
                let now = Instant::now();
                self.last = Some(state.clone());
                self.last_good = Some(now);
                let age = arrived.map(|t| now - t);
                if let Some(age) = age {
                    self.ages.push_back(age.as_secs_f64() * 1000.0);
                    if self.ages.len() > 240 {
                        self.ages.pop_front();
                    }
                }
                match age {
                    Some(a) if a > STALE => LiveStatus::Stale(state),
                    _ => LiveStatus::Fresh(state),
                }
            }
            None => {
                let frames_move = arrived.is_some_and(|t| t.elapsed() < STALE);
                if frames_move && self.last_good.is_none_or(|t| t.elapsed() > REDISCOVER) {
                    self.generation = None;
                    self.last_good = None;
                    return self.searching("mirrored bytes stopped decoding, searching again");
                }
                self.last.clone().map_or_else(|| self.searching("decoding"), LiveStatus::Stale)
            }
        }
    }

    fn poll_search(&mut self) -> LiveStatus<T> {
        if let Some(h) = self.search.take_if(|h| h.is_finished()) {
            let found = h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("discovery panicked")));
            match found.and_then(|regions| {
                let n = regions.len();
                match ShimClient::connect(dxgi_port())?.ask(&Request::Mirror { regions })? {
                    Response::Mirror { generation } => Ok((generation, n)),
                    other => bail!("unexpected reply {other:?}"),
                }
            }) {
                Ok((generation, n)) => {
                    self.generation = Some(generation);
                    self.note = format!("mirroring {n} regions");
                    return self.last.clone().map_or_else(|| self.searching("waiting for the first mirrored frame"), LiveStatus::Stale);
                }
                Err(e) => {
                    self.note = format!("{e:#}");
                    self.retry_at = Some(Instant::now() + RETRY);
                }
            }
        }
        if self.search.is_none() && self.retry_at.is_none_or(|t| Instant::now() >= t) {
            let d = Arc::clone(&self.discover);
            self.search = std::thread::Builder::new().name("grace-live-discover".into()).spawn(move || d()).ok();
            self.retry_at = None;
            self.note = "searching game memory".into();
        }
        let note = self.note.clone();
        self.last.clone().map_or_else(|| LiveStatus::Searching(note), LiveStatus::Stale)
    }

    fn searching(&mut self, why: &str) -> LiveStatus<T> {
        self.note = why.to_owned();
        LiveStatus::Searching(why.to_owned())
    }

    fn arrived(&self, seq: u64) -> Option<Instant> {
        let a = self.arrivals.lock().ok()?;
        a.frames.iter().rev().find(|(s, _)| *s == seq).map(|(_, t)| *t)
    }

    /// Game frames per second over the last second of arrivals.
    pub fn game_fps(&self) -> f64 {
        let Ok(a) = self.arrivals.lock() else { return 0.0 };
        let now = Instant::now();
        a.frames.iter().filter(|(_, t)| now - *t < Duration::from_secs(1)).count() as f64
    }

    /// Sample age at use, ms: (p50, p99) over the last few seconds.
    pub fn age_ms(&self) -> Option<(f64, f64)> {
        if self.ages.is_empty() {
            return None;
        }
        let mut v: Vec<f64> = self.ages.iter().copied().collect();
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        Some((at(0.5), at(0.99)))
    }

    /// Shim copy time per frame, µs.
    pub fn shim_us(&self) -> f64 {
        f64::from(self.copy_ns) / 1000.0
    }

    pub fn note(&self) -> &str {
        &self.note
    }
}

/// Where hosts and `xtask oracle run` put the mirror file of a game.
pub fn mirror_file(local_dir: &Path, game: &str) -> std::path::PathBuf {
    local_dir.join("run").join(game).join("mirror.bin")
}

#[cfg(test)]
mod tests {
    use super::*;
    use grace_proto::mirror::FrameInfo;

    /// A writer standing in for the shim: maps the same file read-write.
    fn writer(path: &Path) -> memmap2::MmapMut {
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).unwrap();
        f.set_len(mirror::SIZE as u64).unwrap();
        unsafe { memmap2::MmapMut::map_mut(&f).unwrap() }
    }

    #[test]
    fn reader_sees_writer_through_the_file() {
        let dir = std::env::temp_dir().join(format!("grace-live-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mirror.bin");
        let mut w = writer(&path);
        let r = MirrorReader::open(&path).unwrap();
        assert!(r.snapshot().is_none());
        let regions = [MirrorRegion { addr: 0x10, len: 4 }];
        let offsets = mirror::layout(&regions).unwrap();
        let info = FrameInfo { frame: 3, generation: 1, ..Default::default() };
        unsafe { mirror::write(w.as_mut_ptr(), &regions, &offsets, &info, |_, d| { d.copy_from_slice(&[1, 2, 3, 4]); true }) };
        let s = r.snapshot().unwrap();
        assert_eq!((s.info.frame, s.regions[0].bytes.as_slice()), (3, &[1u8, 2, 3, 4][..]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mirror_file_layout() {
        assert_eq!(mirror_file(Path::new("/l"), "g"), Path::new("/l/run/g/mirror.bin"));
    }
}
