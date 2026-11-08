# Live mirror (C2-b, milestone B0): design

Date: 2026-11-08. Status: design approved in conversation; spec under review.

## Goal

While the original game runs under the oracle, a native arm64 window (our renderer) follows the game camera in real time. The channel that carries the camera is generic: it mirrors arbitrary regions of game memory every frame, so the same mechanism later carries objects (B2) and bones (B3).

Success:
1. Our window follows the game camera every game frame: the user turns the camera in the game and sees ours follow smoothly.
2. Numbers: game FPS (from the mirror), our FPS, sample age when we render it (p50/p99), shim cost per `Present`. Targets: sample age p99 < 20 ms; shim cost < 50 µs per frame (no measurable game FPS drop).
3. An automatic frame comparison with the oracle at a live camera, as in M6.

Out of scope: loading the world around the player (B1), objects, animation, lighting. B0 uses a fixed map chosen by the port.

## Principle: the engine knows no game

From this milestone on, engine interfaces are designed to serve any game: the engine owns mechanisms (channel, session, policies, stats, rendering), a game repository owns knowledge (where things live in memory and how to decode them) and plugs it in through a small adapter trait. Nothing in `elder-grace` names a game structure, offset or convention. Earlier code that does not follow this yet is left as it is until it is touched.

## Architecture

```
game process (Wine, x86_64)                    native process (arm64)
┌─────────────────────────────┐                ┌───────────────────────────────────────┐
│ dxgi-proxy                  │   mirror file   │ grace-live                            │
│  Present → copy regions ────┼──► (mmap) ─────►│  MirrorReader → LiveSession<A>        │
│  control TCP 47321          │◄── Mirror{…} ───┤  ShimClient     │  A: LiveAdapter     │
│ graphics-proxy (d3d12)      │◄── FindFloats/ ─┤                 ▼  (game repo)        │
│  memory search, TCP 47322   │    Peek         │            LiveCamera → grace-render  │
└─────────────────────────────┘                └───────────────────────────────────────┘
```

### Engine (`elder-grace`)

**`grace-proto`: the mirror record** (no platform dependencies, shared by the shim and the host).
- `Request::Mirror { regions: Vec<MirrorRegion> }` (`MirrorRegion { addr: u64, len: u32 }`), answered with `Response::Mirror { generation: u32 }`. An empty list stops mirroring.
- The mirror file is a fixed 64 KiB record:

| Offset | Field |
|---|---|
| 0 | magic `GRMIRROR` (8 bytes) |
| 8 | `u32` version (1), `u32` region count |
| 16 | `u64` seq: odd while the shim writes, even when stable |
| 24 | `u64` frame: `Present` count |
| 32 | `i64` QPC at the copy, `i64` QPC frequency |
| 48 | `u32` copy time, ns; `u32` generation (bumps when the region set changes) |
| 64 | region table, 64 entries × { `u64` addr, `u32` len, `u32` status (0 ok, 1 read failed) } |
| 1088 | region data, packed in table order, each start 16-byte aligned |

- Limits: at most 64 regions, at most 64 KiB − 1088 bytes of data; the shim rejects a larger request with an error.
- `mirror::write(buf, …)` and `mirror::read(buf) -> Result<Snapshot, Torn>` implement the seqlock: the writer bumps seq to odd, writes, bumps to even (release); the reader reads seq, copies, reads seq again, retries if odd or changed. Both work on a byte slice so they are tested without a shim.

**`dxgi-proxy`: the writer.** It already hooks `Present`, which is the frame boundary.
- `GRACE_MIRROR` (a Windows path) names the mirror file; without it mirroring is off. The shim creates the file, sizes it to 64 KiB and maps it (`CreateFileMapping` + `MapViewOfFile`).
- The `Mirror` request (port 47321) replaces the region set. `Present` takes the set with `try_lock` and never blocks the game: if the set is being replaced, that frame is skipped.
- Each region is read with `ReadProcessMemory` on the current process: a freed object gives status 1, never a crash.
- The shim measures its own copy time and writes it into the header.

**New public crate `grace-live`** (host only; released with `grace-core`, `grace-proto`, `grace-render`).
- `ShimClient`: TCP JSON client with timeouts for both shim ports (`ask(&Request) -> Result<Response>`). `grace-ctl` and ports use it instead of their own clients.
- `MirrorReader::open(path)`: maps the file read-only (`memmap2`); `snapshot() -> Option<Snapshot>` (latest stable record, regions as byte slices with status).
- `trait LiveAdapter: Send + 'static`, the game's part:
  ```rust
  type State: Clone + Send;
  /// Find what to mirror (may take seconds: memory walks). Runs on a background thread.
  fn discover(&mut self, shim: &mut ShimClient) -> anyhow::Result<Vec<MirrorRegion>>;
  /// Decode one snapshot; None when the bytes do not pass the game's validation.
  fn decode(&self, snapshot: &Snapshot) -> Option<Self::State>;
  ```
- `LiveSession<A>`: owns the adapter, the reader and the control client, and applies the policy:
  - start: `discover` on a background thread, then `Mirror { regions }`;
  - every render frame `latest() -> LiveStatus<A::State>` (`Fresh(state)`, `Stale(state)` when seq has not moved for 250 ms (game paused or minimized), `Searching`);
  - rediscovery when `decode` fails for 1 s while seq moves, or every region reports status 1;
  - a watcher thread polls seq at 1 kHz and stamps each new frame with native monotonic time, so sample age is measured without relating Wine's QPC to the host clock (error ≤ 1 ms);
  - stats: game FPS (seq rate), sample age p50/p99, shim copy time; `stats_line()` for a window title and an optional CSV log (`frame,t_ms,age_ms,copy_us`).
  - The control side is a trait (`MirrorControl`) so the policy is tested with a fake adapter and a fake shim.
- `LiveCamera` (engine convention: right-handed, Y up, camera looks down −Z): `view_to_world: Mat4`, `fovy`, `near`, `far`, `aspect`. Converting from a game's convention is the adapter's job.

**`grace-render`.**
- `Camera` gains an explicit-view form (`Camera::from_view(&LiveCamera)`), so roll and exact matrices survive (the orbit form stays for the free camera).
- `viewer::run` takes an optional camera driver (`FnMut() -> Option<(Camera, String)>`) polled every frame: when it returns a camera, the viewer uses it and the string as the window title; in that mode the viewer redraws continuously. Mouse and keys still work when the driver returns `None` (free camera while searching).

**`xtask oracle run`** sets `GRACE_MIRROR` to `<local_dir>/run/<game>/mirror.bin` (as a `Z:` path) and creates the directory. `grace_core::paths` gets `mirror_file(game)` so hosts find the same file from the config.

### Reference adapter (`grace-eldenring`)

- New crate `er-live`: `EldenRingCamera: LiveAdapter<State = LiveCamera>`. `discover` is the current `er-tool camera` search moved here (cbSceneParam camera, then the game's camera structure by [fovy, near, far, aspect], copies chosen by voting); it returns the 3–4 best copies as regions. `decode` validates (orthonormal rotation, sane fovy), applies the origin shift and converts to the engine convention. Layout knowledge stays in `docs/FORMATS.md`.
- `er-tool camera` uses `er-live` (same output as today).
- `er-viewer <map> --map --live` opens a `LiveSession` and drives the viewer camera; window title: `game 58 fps · ours 120 fps · age 6/11 ms · shim 9 µs`.

## Risks and the first check

- **Is a Wine file mapping visible to a native mmap at once?** Wine maps writable shared file views with `MAP_SHARED`, so it should be. The first implementation task checks it with the real shim. Fallback: the shim writes the record with `WriteFile` at offset 0 every frame; the page cache is shared, so the reader is unchanged.
- **Camera structure copies may move** (loading screens, fast travel): handled by the rediscovery policy.
- **Discovery takes ~12 s** (the memory walk): acceptable once per session; the viewer shows the free camera and "searching" meanwhile.

## Testing

- `grace-proto`: record roundtrip; torn-read detection (odd seq, seq changed during the copy); region limits.
- `grace-live`: `MirrorReader` on a temporary file written by a writer thread; `LiveSession` policy (fresh → stale → rediscovery) with a fake adapter and a fake `MirrorControl`.
- `er-live`: decode of a synthetic crypt sample (local (0.91, 7.27, 6.18), shift (112, −8, −8) → world (−111.09, 15.27, 14.18)).
- Shim: Windows-target clippy; live run with the game (user in the Stranded Graveyard crypt): numbers into `PROGRESS.md`, an M6-style comparison frame, the user's check by eye.

## Branches

`feat/live-mirror` in both repositories. ADR 0006 records the channel decision.
