# Progress log

Newest entries on top. Each entry: what was done, how it was checked, numbers, what is next, blockers. Entries before Phase R were written in Russian and translated to English during Phase R.

## ▶ Current state (summary as of 2026-11-08, read first)

**Where we are:** path A (native data viewer) is complete, 6/6 milestones. **Now:** Phase R (repository restructuring, READMEs, English), branch `chore/phase-r` in both repositories. **Next:** Phase 6 **C2-b** (native rendering driven by the live game's state). Plan: `docs/PLAN.md` ("Phase R", "Phase 6: C2-b"); decisions: ADR 0004 (C2-b), ADR 0005 (engine as a framework, cargo git dependency + `[patch]`). **Do not start C2-b without the user's go.**

**Layout:** `ProjectPort/` (not git) → `elder-grace/` (engine, git, remote `github.com/OsipShmel/elder-grace`), `grace-eldenring/` (game, git, remote `github.com/OsipShmel/grace-eldenring`), `.local/` (data, prefix, caches, frames; not git). Pristine Elden Ring 1.17.2: `~/Games/EldenRing/Game` (read-only); sandbox: `.local/game_bean/elden_ring`.

**What works (verified):**
- **Oracle** on GPTK 3.0-3 in one command: `cargo xtask oracle setup|steam [--ui]|run [--label L] [--no-shim] [--no-overrides] [--no-esync]|status|kill|shot <png>`. esync on by default (+~20% in the open world). Steam logs in by itself with the saved session.
- **d3d12 shim in the game:** shader dump (5988), live overrides (`grace-ctl overrides on|off|toggle|reload`, `cargo xtask overrides --reload`; `.hlsl` for DXBC/DXIL, `.irpatch` patches the original DXIL), `grace-ctl bound` (shaders of the frame), camera and memory debugging: `grace-ctl camera|peek|find-floats` (process memory walk via VirtualQuery + ReadProcessMemory, safe). Only the "golden text" override is active; demo patches are in `grace-eldenring/shaders/overrides/examples/`.
- **Measurements:** dxgi shim (frame time), `cargo xtask bench`, `cargo xtask trace [--template "System Trace"]`.
- **Data layer (`grace-eldenring/crates/er-data`):** RSA keys from the user's `eldenring.exe` (never in git), BHD5/BDT/AES, DCX (Kraken in pure Rust), BND4, FLVER2, MATBIN, TPF, MSB. `er-tool keys|ls|find|probe|extract|flver-check|camera`.
- **Native renderer (`elder-grace/crates/grace-render`, `grace-eldenring/crates/er-viewer`):** Metal, instancing, BCn textures, alpha test, camera (orbit + WASD), PNG capture, "ours | oracle" side by side. The whole Stranded Graveyard scene (`er-viewer m18_00_00_00 --map`), ~1 s to load, 31 M triangles.
- **M6, automatic comparison with the oracle:** game in the crypt → `cd grace-eldenring && cargo run -p er-tool -- camera --save cam.json` → `cd ../elder-grace && cargo xtask oracle shot oracle.png` → `cargo run --release -p er-viewer -- m18_00_00_00 --map --camera cam.json --capture ours.png --size 1728x972 --compare oracle.png`. Match within ~1–3% of the frame width.

**Key knowledge (details in `grace-eldenring/docs/FORMATS.md`, `BASELINE.md`):**
- The bottleneck is the CPU (main thread under Rosetta + D3DMetal translation), GPU ~67%. The game's shaders already run natively on the GPU, so we do not push our own MSL into the game process.
- Camera: `cbSceneParam` (pos +48, proj +256, invView +320) in **local** coordinates; world coordinates come from the game's camera structure (row-major matrix + origin shift, e.g. (112, −8, −8), + [fovy, near, far, aspect]).
- **Never:** replace D3DMetal resource vtables (the game crashes), put ABI service words into vtable copies (crashes). Vtable copies keep the plain layout (original at [−1]).

**Renderer gaps:** SpeedTree trees (positions in a foreign stream), bones of static meshes, normals/lighting/fog as in the game, mask textures of map pieces, the open world (`m60`).

**Principles:** the user's legal copy; pristine is read-only; no game data in git; nothing online; logins by the user; external source builds only with their "yes"; large work in feature branches (the agent commits; merges and pushes only with the user's permission); conversation in Russian; docs, code and commits in English.

---

## 2026-11-08 — Session 8: Phase R (branch `chore/phase-r`, both repositories)

**Done so far:**
- `feat/game-camera` merged into `main` in both repositories (fast-forward, with the user's permission).
- Engine code translated to English: comments, doc comments, error/log/CLI messages, manifests. `xtask` no longer defaults to `eldenring`: the game comes from `GRACE_GAME` or the only `[games.*]` entry; `oracle shot` reads `window_title` from the game config (new `GameConfig::window_title`). Windows-target clippy fixed (crash-probe, compat-realtime).
- Engine docs translated (ADRs 0001–0005, ARCHITECTURE, PLAN, REFERENCES, THIRD_PARTY, games/README, this log).

- Engine: README (framework intro, crates, how a port depends on it), LICENSE (MIT), CHANGELOG, `repository` fields; local tag `v0.1.0` on `0427e75` (not pushed).
- Port: `grace-core/proto/render` via `git = "https://github.com/OsipShmel/elder-grace", tag = "v0.1.0"`; local co-development through `ProjectPort/.cargo/config.toml` (`[patch]`, outside git; no unused-patch warning in the engine workspace). README, LICENSE, CLAUDE.md, all docs and code in English. `Cargo.lock` is committed in the patched (local) form: with the patch active cargo rewrites a git-pinned lock anyway; the tag in `Cargo.toml` is the pin.
- Public-repo hygiene: history of both repositories rewritten with `git filter-repo` (before the first push) to drop mentions of an unrelated installer copy, Steam session file details and the local user path; commit hashes changed, `v0.1.0` now points to `0427e75`.

**Checked:** `cargo test --workspace` green, `cargo clippy --workspace --all-targets` clean, Windows clippy of the shims clean, `cargo xtask shim --release` builds. Clean clones of both branches outside ProjectPort (GitHub URL redirected to the local repo with `url.insteadOf`): engine 20 tests, clippy 0; the port fetches the engine by tag `v0.1.0` (tag commit), 22 tests (without the install and with `GRACE_ROOT` on real files), clippy 0; `er-viewer m18_00_00_00 --map --capture` renders the map (1.0 s, 11 054 instances). `cargo xtask oracle status` works. **Not checked:** `oracle run` (needs the game started).

**Next (user):** create both GitHub repositories (public), merge `chore/phase-r` into `main` in both, push `main` and the tag `v0.1.0` of the engine first, then the port. After that Phase R is done; C2-b starts on the user's go.

---

## 2026-11-08 — Session 7: camera from the game, M6 (branches `feat/game-camera`, both repositories)

**Done:**
- `grace-proto`: `Request::Camera { layout }`, `CameraReport/CameraSample`, `camera_at`: a self-validating signature (view-to-world translation = position or 0; orthonormal rotation; perspective projection, fovy 10°..150°; screen size in whole pixels; both matrix orders) + tests; `Request::Peek` (read game memory) and a projection list for debugging without restarts.
- d3d12 shim (`resources.rs`): walking writable process memory (VirtualQuery + ReadProcessMemory, safe), ~7 GB in ~12 s.
- `grace-ctl camera --layout … --save cam.json`, `grace-ctl peek <address>`; `cargo xtask oracle shot <png>`: a screenshot of the game window by title.
- `grace-render`: `Camera::looking`, `compare::side_by_side` (with margin cropping); `er-viewer --camera cam.json [--offset x,y,z] --compare game.png`.
- `grace-eldenring/data/camera-layout.json`: the `cbSceneParam` layout.

**What did not work (important not to repeat):**
- Replacing D3DMetal **resource** vtables (hooking Map): the game crashes at startup, D3DMetal recognizes its resources by their vtable.
- Vtable copies that preserve the ABI service words ([-2], [-1]) with the original address at [-3]: also a crash (probably virtual inheritance). The old layout (original at [-1]) stays; it works.
- `cbSceneParam` in GPU upload buffers is not visible to the CPU through the Windows API; copies are found on the main thread's stack.

**How it was checked (user in the game, 15:30–16:00):**
- The camera was found in the open world and in the crypt: fovy 48°, frame 1728×972, matching the visible picture (window margins); coordinates are local (|p| < 10).
- The Stranded Graveyard crypt at site of grace `AEG099_060_9001`: our frame (Metal, native) and the game frame from the same camera match: vaults, arches, columns, urns on the left, sarcophagus and statue on the right, torches (`.local/captures/m6-final-vs.png`). The local → map shift (−114.62, 8.23, 6.86) was fitted by eye (first from the grace position, then visually).

**World origin found (same session):** the second pass (matrices with the same rotation) found the world copy of the camera, and `peek` around it revealed the game's camera structure: row-major matrix + **origin shift (112, −8, −8)** + [fovy, near, far, aspect]. `er-tool camera` (game repository) finds it via `FindFloats` by fovy/aspect, reads it with `Peek`, world = local − shift, voting across copies (32/32). A manual `--offset` is no longer needed; the comparison matches within ~1–3% of the frame width (columns, torches, statue, arch): `.local/captures/m6c-ours-vs.png`.

**Reproduce (all commands):** game in the crypt → `cd grace-eldenring && cargo run -p er-tool -- camera --save cam.json` → `cd ../elder-grace && cargo xtask oracle shot oracle.png` → `cargo run --release -p er-viewer -- m18_00_00_00 --map --camera cam.json --capture ours.png --size 1728x972 --compare oracle.png`.

**Next:** merge the `feat/game-camera` branches (both repositories together); then lighting/fog/exposure from `cbSceneParam` (they live there too), SpeedTree trees, bones of static meshes; the open world by `m60` tiles (the camera already works there).

---

## 2026-11-08 — Session 6: path A, M1 and M2 (`grace-eldenring`)

**Decision:** after an honest review ("the oracle and override tools are ready, but the game is entirely translated, shaders are bound by D3DMetal and the GPU is not the bottleneck") the user chose path A: our own native runtime on top of the game data. Spec and plan: `grace-eldenring/docs/{specs,plans}/`.

**Done (workspace `grace-eldenring/`, crates `er-data`, `er-tool`):**
- M1: RSA keys from the user's `eldenring.exe` (not in the repository), decryption and parsing of all 7 `.bhd` files (~130k entries), path hashing, a `Game::open/find` index, `er-tool keys|ls|find`.
- M2: reading `.bdt` + AES-128-ECB by ranges, DCX (KRAK via pure-Rust `oozextract`, DFLT, ZSTD), BND4, `er-tool extract|probe`.
- Found: Stranded Graveyard is MSB `m18_00_00_00` + two map pieces + ~4800 placements of `AEG…` assets (`/asset/aeg/…geombnd.dcx`, FLVER + LOD).

**How it was checked:** 15 `er-data` tests (real install files, skipped without it): every entry of every archive fits into its `.bdt`; the MSB decompresses (KRAK 159 KB → 6.9 MB, `MSB ` signature); msgbnd → 26 `.fmg`; FLVERs of a map piece and of asset `AEG001_290` extracted. clippy clean.

**Not done from the plan:** a differential check against `fromsoft-formats` (GPL, dev-dependency), postponed until FLVER where errors are more likely; M1–M2 formats validate themselves (bounds, signatures, sizes).

**Session 6 continued, the first picture:**
- `er-data/flver.rs`: FLVER2 (version 0x2001A): materials, meshes, positions/normals/UVs, LOD0 indices; test: vertices inside the bbox, unit normals, indices in range. Elden Ring FLVERs only carry texture slot names (`…_AlbedoMap`); the paths come from the material's MATBIN.
- `elder-grace/crates/grace-render` (core): meshes in Shared buffers, memoryless depth, Lambert + sky, orbit camera, winit window, `capture_png`.
- `grace-eldenring/crates/er-viewer`: a model from the archives → window/PNG. Frames (checked by eye): AEG001_290 (vegetation cards), **AEG003_055 (sacks on a stretcher)**, AEG003_082 (poles with sacks); map piece `m18_00_00_00_000000` ("暗室") is a rough service shell.

**M3/M4 (same session):** `er-data`: MATBIN, TPF; `grace-render`: DDS → BCn as is (all mips), UVs, sampler, alpha test; `er-viewer`: material → MATBIN → albedo (or the path from FLVER) → TPF. Frames (checked by eye): sacks on a stretcher, firewood with sacks, a bush (BC7 + alpha), with the game's textures. Not covered: textures of `m18_…` map pieces, normals/other slots, lighting "as in the game".

**M5 (same session):** `er-data/msb.rs` (349 models, 4683 parts, 4533 assets), `grace-render`: instancing and WASD/QE flight, `er-viewer --map`: 4535 parts → 977 meshes, 122 textures, 11 054 instances, 31 M triangles, ~1 s to load (release). Part transform T·Ry·Rz·Rx·S: the walls line up, so the order is right. Frames (checked by eye): gothic walls with arches, vaults, corridors, columns with the game's textures.
Known gaps: SpeedTree trees (`AEG801_*`: positions in a foreign stream, 31 meshes not drawn), 4 `AEG099_*` models not found, `AEG099_720` has vertices in bone space (bones not applied), textures of map pieces `m18_00_9xx` (masks without albedo), lighting/fog/normals as in the game.

**Next:** M6: comparison with the oracle needs the camera from the live game → a separate injection branch (the user's decision). Possible without the game: SpeedTree trees, bones of static meshes, normals.

---

## 2026-11-08 — Session 5: analyzing waits → esync (branch `feat/dxil-overrides`)

**Done:**
- `cargo xtask trace --template "System Trace" [--instrument I]` (commit `5609afb`).
- A 15 s trace in the open world (user in the game), `thread-state` export (2.2 GB of XML) and `time-profile`, summarized with one-off scripts (in the scratchpad; move them into `grace-bench` if repeated).
- Finding: the main thread is 66% running / 24% blocked / 10% waiting for a core (on Super cores); **22% of its running time is `wine_server_call`** (`NtSetEvent`, `NtResetEvent`, `NtReleaseMutant`, `NtCreateEvent`), 45–59% for five worker threads. Details: `grace-eldenring/docs/BASELINE.md`.
- GPTK's `ntdll.so`/`wineserver` are built with esync → `oracle`: `WINEESYNC=1` by default, `--no-esync`, a check of the running wineserver's mode (otherwise Wine refuses to start the process), `status` shows the mode (commit `8c846a7`).

**How it was checked:** A/B at a site of grace, 20 one-second samples: without esync 49.6 FPS (46.8–54.6) → with esync 59.1 (55.0–60.0, the game's cap); Steam and the game start with esync without errors. The view in the second sample was heavier (1744 draws vs 903), so +19% is a lower bound. Open world, 298 s of running: **47.1 FPS vs 38.7** for world-base, p95 **26.6 vs 40.0 ms** (different routes); trace: GPU 67%, main thread 80% of a core.

**Next:**
- ~~Retake the open-world baseline with esync~~: done (`world-esync`), see BASELINE. Next analysis: a System Trace with esync: where the main thread's 80% goes (Wine unix code, D3DMetal, Rosetta) and the remaining waits.
- If the 60 FPS cap gets in the way of measuring: find the community's standard way to lift the FPS limit offline (sandbox only), or measure GPU/CPU frame-time headroom.
- Not checked: msync (CrossOver) vs esync; GPTK's Wine has no msync.

---

## 2026-11-08 — Session 4: DXIL overrides (branch `feat/dxil-overrides`, on top of `feat/live-overrides`)

**Done:**
- dxc v1.9.2609 built from source (the user's permission: "build dxc"), `dxc`/`dxa`/`dxv` + `libdxcompiler.dylib` in `.local/tools`; recipe in ARCHITECTURE, license in THIRD_PARTY.
- `grace-shader`: ISG1/OSG1 signatures (DXIL), debug names from `ILDN`, `names <dump> [substring]`; a test on a synthetic container.
- `xtask/src/overrides.rs`: `.hlsl` → vkd3d or dxc depending on the original; **`.irpatch`**: patching the original DXIL without sources (dumpbin → `storeOutput` × constant → dxa → dxv); signature checks; removal of stale overrides; `cargo xtask shader <id> [--ir]`. 2 patcher tests.
- d3d12 shim + `grace-ctl bound [--ms] [--ps]`: which shaders were bound during a window (via PSO recipes).
- `grace-eldenring`: `shaders/overrides/1f57f44ecec2766b.ps.irpatch`, a cold tint for `ToneMap_TabledToneMapPS` (commit `4b832c3` in the game's `main`); FORMATS: shader debug names.

**How it was checked (without the game):**
- dxc reads the game's DXIL (`-dumpbin`), HLSL → DXIL is signed (non-zero hash); the test DXIL and the **patched game tone mapping** pass Metal Shader Converter (`grace-irconv`) and load natively in Metal (M5 Pro, 1/1). D3DMetal uses the same converter, so the game was expected to accept it.
- The "disassemble → dxa → dxv" round trip on the tone mapping: `Validation succeeded`, signatures match.
- `cargo test -p grace-shader -p xtask -p grace-proto` green, clippy on the changed crates clean.

**Checked in the game (2026-11-08, 13:10–13:30, Steam logged in by itself with the saved session):**
- Title screen: `grace-ctl bound` shows ~6 PSOs; no tone mapping there, but a DXIL `GXSimple_BlendUI_SDR2SDR_F` (the last pass: UI over the frame).
- An `.irpatch` on it (red) + on the tone mapping (cold) → `cargo xtask overrides --reload` in the live game: "PSOs recreated 5, errors 0": **D3DMetal accepts patched DXIL**. In the world (the user was playing, catacombs) `off`/`on` screenshots: a normal frame ↔ the whole frame including UI in a red-brown tone (both patches). PSO pairs 8, failed 0.
- Not analyzed: on the title screen the `BlendUI` patch gave no visible effect (screenshot right after reload, Direct mode).
- **A/B of the hook cost** in the world: 6 cycles × (10 s on, 10 s off), FPS from the dxgi shim once a second (excluding the first 2 s of each window), 48 samples each: on 49.6 FPS (median 49.3), off 46.2 (47.0). The difference comes from scene changes (the player was moving); **the cost of hooking `SetPipelineState` is not visible above the noise**. A strict measurement needs two route runs and `bench`.
- The demo DXIL patches moved to `grace-eldenring/shaders/overrides/examples/` (not built by default); only "golden text" is active (in the world it is the HUD text, 7 PSOs).

**Next (proposal):** merge the branches; then the first useful overrides instead of tints: candidates from the GPU profile (what is expensive in a world frame: `cargo xtask trace` + `grace-ctl bound`), and a CPU/GPU wait analysis (postponed, needs a run in the world).

---

## 2026-11-08 — Session 3: live overrides (branch `feat/live-overrides`)

**Done** (2 agent commits in the branch, not merged at the time):
- `19e7f4f`: "original → override" PSO pairs, a command list vtable copy (`SetPipelineState` 25, `Reset` 10, `ClearState` 11, `QueryInterface` 0; `CreateCommandList` 12 and `CreateCommandList1` 51, checked against mingw-w64 `d3d12.h`), a TCP 47322 control channel, `grace-proto` `Overrides{op}` + `OverrideStatus`, `grace-ctl overrides`. Overridden descriptions get CachedPSO zeroed; if the override PSO fails to create, the game keeps the original (previously it would have received an error).
- `aecdf3a`: PSO recipes (`recipes.rs`) and `overrides reload`, `cargo xtask overrides --reload`.

**How it was checked (by the agent, in the game, title screen, `oracle run --label live-ov-2`):**
- `grace-ctl overrides`: 1 file, 1 PSO pair, 8888 recipes, substitutions growing (~30/s).
- `off` → the substitution counter stops, the "PRESS ANY BUTTON" text is white; `on` → golden again and the counter grows (`screencapture` screenshots, checked by eye).
- Recolored the HLSL to light blue → `cargo xtask overrides` + `reload` → the text is light blue (recreation 89 ms, the first time includes reading the dump); restored the file → `reload` → golden (2 ms). Removed the override `.dxbc` → `reload` → "removed 1", 0 pairs; restored → the pair is back.
- clippy on the changed crates clean, the protocol test (`overrides_wire_format`) green.

**Not checked / limitations:**
- The cost of hooking `SetPipelineState` in the world (thousands of calls per frame): needs a user run.
- Bundles recorded before a toggle keep the previous PSO until the game rewrites them.
- Recipes hold a reference to all ~8900 PSOs (a leak if the game recreates pipelines; Elden Ring loads its library once), only with `GRACE_SHADER_OVERRIDE` set.
- The `applied` list in the status is overrides applied at least once (not cleared after removal).
- Unrelated to the branch: `cargo clippy --workspace` gave 13 `transmute used without annotations` warnings in `tools/grace-irconv` (a new toolchain lint; fixed later); the repository as a whole is not `rustfmt`-formatted (251 diffs before the branch): a separate small task.

**Next:** merge by the user → `feat/dxil-overrides` (building dxc needs the user's "yes" to build third-party sources) → DXIL shader overrides (99.5% of the corpus).

---

## 2026-11-08 — Session 2: measurements (phase 1), tools (in main)

**Done** (commits in the engine's `main`, made by the agent with the user's permission):
- `shim/dxgi-proxy` → `dxgi.dll`: hooking `Present`/`Present1` through factory and swapchain vtable copies, a CSV frame-time log, stats over TCP 47321.
- `crates/grace-bench` + `cargo xtask bench`: a report from the log (FPS, p50/p95/p99, 1%/0.1% low, stutters, histogram) and a comparison of two runs.
- `grace-ctl --shim stats|watch` (TCP to the shim in the game).
- `cargo xtask trace`: a Metal System Trace of the running game + analysis (GPU occupancy, per-thread CPU, verdict).
- `oracle run --label` (run label); `--no-shim` now removes only the d3d12 proxy.

**How it was checked / numbers (menu only, not a baseline):**
- log: 5743 frames in 134 s, average 42.9 FPS, p95 30.7 ms, 1% low 6.0 (loading and menu transitions);
- menu trace (the window possibly in the background): GPU busy 28–35%, main thread 55–64% of a core, process ~2.9 cores → verdict "waiting", neither GPU nor CPU saturated;
- 5 grace-bench tests, clippy clean.

**Next (needs the user):** a baseline in the world: a 1–2 min route, `cargo xtask oracle run --label world-base`, `cargo xtask trace --label world-base` during the run, then `cargo xtask bench world-base`. The game window must be active (macOS throttles background windows).

---

## 2026-11-08 — Session 1: the shim inside Elden Ring, the first game shaders in native Metal

**Done (phases 2–3, first steps)**
- `shim/graphics-proxy` → `d3d12.dll`: a D3DMetal proxy inside the game process (Windows ordinals, override `d3d12=native,builtin` for the game exe in `oracle setup`). Pipeline creation hooked through a **vtable copy** of the device (the original D3DMetal vtable is in read-only memory of the macOS framework and Wine's `VirtualProtect` does not change that; the first attempt silently failed). Shader dump into `.local/shaders/eldenring/`.
- `crates/grace-shader`: DXBC/DXIL container parsing, `inventory`, `info`, `metallib` (native load in Metal). 3 tests (synthetic container, garbage, the real corpus if present).
- `tools/grace-irconv` (x86_64-apple-darwin, under Rosetta): DXIL → metallib through `libmetalirconverter.dylib` from GPTK.
- `cargo xtask shaders [game]`: summary → conversion → check in one command. `oracle`: a `GRACE_WINEDEBUG` variable for Wine channels; `status` sees the game process.

**How it was checked / numbers**
- Shim log: `D3D12CreateDevice(0xc000) → 0`, `CreatePipelineLibrary(451014 bytes)`, pipelines graphics 46 · stream 1.
- Menu corpus: 54 shaders (DXBC 5.0: 11 ps + 18 vs; DXIL 6.0: 11 ps + 12 vs + 1 hs + 1 ds), 371 KB, 0 unrecognized.
- DXIL → metallib: 25/25; Metal (Apple M5 Pro): libraries loaded 25/25, functions 29, errors 0.
- clippy clean, all tests green. The game did not crash; only the Steam overlay did (`gameoverlayui`: `ClientAPI_Init` assert → crash in dbghelp), which the oracle does not need.

**DXBC branch (after the user allowed building vkd3d):**
- vkd3d 1.19 built into `.local/tools` (recipe and pitfalls in `ARCHITECTURE.md`: an old system bison, a full `make` is needed, `configure` requires MoltenVK).
- `cargo xtask shaders`: DXIL 25/25 through Metal Shader Converter + DXBC 29/29 through vkd3d → SPIRV-Cross → `xcrun metal`. **Result: Metal (Apple M5 Pro): libraries 54/54, functions 58, errors 0.**
- This is compilability, not rendering: D3D12 → Metal resource bindings are not mapped yet.

**Baseline (by eye, the user):** ~40 FPS in the open world and dungeons, ~35 at maximum settings; the shim does not noticeably change FPS (`oracle run` vs `oracle run --no-shim`). Recorded in `grace-eldenring/docs/BASELINE.md`.

**Pipeline library:** in the world there were almost no new shaders, because the game loads PSOs from `ID3D12PipelineLibrary`. The shim got a vtable copy of the library (slots 9/10/13: `LoadGraphicsPipeline`, `LoadComputePipeline`, `LoadPipeline`). Result of one run to the menu: 8874 PSOs from the library, a corpus of 5988 shaders (106 MB), 99.5% DXIL 6.0. The `_stats.txt` summary is now written every 25 pipelines.

**The full corpus through the pipeline** (`cargo xtask shaders`, log `.local/logs/shaders-full.log`): DXIL → metallib 5957/5957, DXBC → metallib 31/31, **Metal (Apple M5 Pro): libraries 5988/5988, functions 6164, errors 0.** Compilability only; D3D12 → Metal resource bindings are still not mapped.

**Shader overrides (C1), first version:**
- Resource bindings solved "the proper way": we replace bytecode at the D3D12 level and D3DMetal produces the Metal code itself; the root signature and PSO state stay the game's. Our own metallib cannot be given to D3DMetal directly: there is no entry point.
- `shim/graphics-proxy/src/overrides.rs`: the `GRACE_SHADER_OVERRIDE` directory (`<fnv64>.<stage>.dxbc`), replacement in a description copy (graphics 656 B, compute 56 B, subobject stream); loading an overridden PSO from the library is reported as a miss (`E_INVALIDARG`).
- `grace-shader info` prints ISGN/OSGN signatures (semantics, registers, masks).
- The first override: the UI font `cfb409a01172e9fb.ps` (DXBC ps_5_0): HLSL in `grace-eldenring/shaders/overrides/`, the same glyph alpha, golden color. `cargo xtask overrides` builds the HLSL through vkd3d; `oracle run [--no-overrides]`.
- Checked: shim log `shader override cfb409a01172e9fb.ps applied`, the game runs, no new crashes. **The user confirmed: the menu text is golden: the first Elden Ring frame partly drawn by our shader.**

**Blockers / not done**
- The corpus was taken from the title menu only; the world needs loading a save (thousands of pipelines).
- Not checked by the user: Cmd+Tab after `CaptureDisplaysForFullscreen = n`.

**Next:** (1) phase 1: frame-time log from the shim (Present hook), a route, Metal System Trace: CPU- or GPU-bound; (2) overrides for DXIL shaders (99.5% of the corpus): needs an HLSL → DXIL compiler (dxc) or DXIL patching; (3) live toggling of overrides through `grace-ctl` for A/B.

---

## 2026-11-08 — Session 0f: Steam's CEF running under GPTK

**Done**
- `shim/crash-probe` (Windows exe): an AeDebug debugger instead of `winedbg`, which GPTK lacks. Registration: `HKLM\Software\Microsoft\Windows NT\CurrentVersion\AeDebug` → `Debugger = C:\grace\crash-probe.exe %ld %ld`, `Auto = 1`. Reports in `C:\grace-crash\<pid>-<tid>.txt`: the exception as `module+offset`, registers, return addresses (heuristic), **strings near the crash**, modules.
- The cause of the `steamwebhelper` crash was found from the strings in the report: an `int3` in `libcef.dll+0x59ef905`, a fatal Chromium check that needs `QueryUnbiasedInterruptTimePrecise` from `api-ms-win-core-realtime-l1-1-1.dll` (Win10+). Wine 7.7 lacks these functions; Chromium looks them up via `GetModuleHandle(<API-set name>)`, and Wine resolves the name to its own `kernelbase`.
- `shim/compat-realtime` → `api-ms-win-core-realtime-l1-1-1.dll`/`-l1-1-2.dll` in the prefix's `system32`: `Query*InterruptTime*`, `Query*CycleTime*`, aux counter → `E_NOTIMPL`.
- `compat-bcrypt` (imported by `steamwebhelper.exe`) now loads compat-realtime and installs a **fallback `GetProcAddress`** into the IAT of `libcef.dll`, `steamwebhelper.exe`, `chrome_elf.dll`: if Wine returns NULL, the function is looked up in the compat DLL (`src/getproc_fallback.rs`).
- The shim DLLs got their own image bases (`build.rs`, `--image-base` 0x6f0000000/0x6f1000000/0x6f2000000): the default `0x180000000` is taken by `libcef.dll`.
- Debugging hacks reverted: AppInit_DLLs off, relay trace keys removed.

**How it was checked:** `steamui_html.txt` → `CreateResponse` / `BrowserReady`, the webhelper no longer restarts, `crash-probe` writes no reports.

**Transport error 0x3007** ("Unexpected transport error" when starting Steam):
- Cause: the client accepts the `steamwebhelper` connection via `AcceptEx`, then calls `getsockname` on the accepted socket; Wine 7.7 does not know the local address of AcceptEx sockets → `WSAEINVAL` (`net.cpp: getsockname failed in BGetBoundAddr with error 10022`). Wine ignores `SO_UPDATE_ACCEPT_CONTEXT`, and Steam even passes the accepted socket itself into it.
- Fix in `compat-bcrypt` (`src/iat.rs`: a generic IAT patcher by name/ordinal, target `*` = all modules, plus patching at load time via `LdrRegisterDllNotification`; `src/winsock.rs`): a `WSAIoctl` wrapper hands out its own `AcceptEx`, which remembers "accepted → listening"; `getsockname` on `WSAEINVAL` returns the listening socket's address (as Windows does). ws2_32 hooks by ordinals 3/6/21 and by the name `WSAIoctl`.
- Checked: 16 address substitutions per run, 0 new `BGetBoundAddr`, `BrowserReady`, no `crash-probe` reports.

**Transport error 0x3009:**
- Cause: the client checks the owner of the websocket connection via `GetExtendedTcpTable(AF_INET, TCP_TABLE_OWNER_PID_CONNECTIONS)`, but Wine 7.7 on macOS returns an empty table (checked with `load-probe.exe tcp-owners`: 0 rows even for its own loopback connection) → `transport_client.txt`: `Connection rejected`.
- Fix `compat-bcrypt/src/tcp_owners.rs`: a TCP registry in prefix-wide shared memory (`Local\ElderGraceTcpOwners`, 1024 slots). `connect`/`listen` hooks (ws2_32, by ordinal and by name) write rows with PIDs; `closesocket` removes them; a `GetExtendedTcpTable` hook (classes 3/4/5, IPv4) appends registry rows to Wine's answer, skipping dead processes.
- Checked: `GetExtendedTcpTable(class 4): Wine 0 rows + registry 2`, 0 new `Connection rejected`, **the Steam login screen appeared** (confirmed by the user).

**Login and the first game launch:**
- The user logged in through the Steam window: `connection_log` → `Logged On`, the session is remembered. With `-silent` there is no main window, which is expected.
- Elden Ring from the sandbox (`steam_appid.txt` = 1245620, the `bcrypt.dll` shim next to it, override for `eldenring.exe`) with Steam logged in: the process lives > 1.5 min, CPU ~135%, 3–5 GB RSS, no `crash-probe` reports.
- **The user confirmed: the Elden Ring title menu renders under GPTK.** Problem: one could not leave the full-screen game (Cmd+Tab) because Wine captures the display; set `HKCU\Software\Wine\Mac Driver\CaptureDisplaysForFullscreen = n` (not checked by the user).

**`cargo xtask oracle`** (`xtask/src/oracle.rs`): `setup | steam [--ui] | run [game] [--no-hud] | kill | status`. `GameConfig` got `exe` and `steam_appid` (xtask knows nothing about games); `[oracle] env = "gptk"`. Checked: `setup` on an existing prefix is idempotent (< 1 s after the build), `status` → `Steam: online`, 0 crash-probe reports; clippy clean, tests green. **Not checked:** `setup` on a fresh prefix and `run` through the command (the game was started by hand).

---

## 2026-11-07 — Session 0e: Steam under GPTK, compat-bcrypt

**Done**
- The cause of the Windows Steam crash under GPTK: `BCryptGenRandom` with pseudo-handle `0x81` (Win10+); Wine 7.7's builtin `bcrypt.dll` dereferences it (crash at `bcrypt.dll+0x390A`, inside `BCryptGenRandom`). `steamclient64.dll` and `tier0_s64.dll` import it statically.
- Wrote `shim/compat-bcrypt` (→ `bcrypt.dll`): 61 exports as naked trampolines into the builtin bcrypt, translation of pseudo-handles 0x01–0xf1 via `BCryptOpenAlgorithmProvider` with a cache. `load-probe.exe bcrypt-rng`: without the shim a page fault, with the shim `status 0x0` and random bytes. `cargo xtask shim` builds it.
- Installed as `Steam/bcrypt.dll` + `AppDefaults\steam.exe\DllOverrides bcrypt=native,builtin` (also for `steamservice.exe`, `eldenring.exe`).
- **Result:** the Steam client core no longer crashes: the self-update passes, the network connects (`connection_log`: Connected).

**Blocker at the time:** the Steam UI (CEF, `steamwebhelper.exe`) did not come up on Wine 7.7 (solved in session 0f):
- without flags the webhelper restarted every ~10 s, `steamui_html.txt`: endless `RetryCreateBrowser`, no explicit crashes;
- with `-no-cef-sandbox -cef-disable-gpu` the webhelper crashed: `int3` in `libcef.dll+0x59ef905` (a Chromium CHECK) and execution at a data address in `libpyrowave-shared-1.dll`.

**Option A (chosen by the user), result:**
- Windows SteamCMD (`C:\steamcmd`, Valve's official zip) also crashed on the pseudo-handle after bootstrapping; `compat-bcrypt` + an override for `steamcmd.exe` fixed it. The user logged in themselves (password + Steam Guard).
- The client's saved login state was set up so that Steam reuses the session; token values were never printed.
- `steam.exe -silent -no-browser` picked up the account but did not start logging in: in the new client login goes through the UI (CEF), which was still broken at that point.
- **Result for GPTK at the time:** the shim, compat shims and SteamCMD work; the Steam client on Wine 7.7 does not log in without a working CEF (fixed in session 0f).

---

## 2026-11-07 — Session 0d: repository split, Metal Toolchain, GPTK

**Done**
- Repositories split (ADR 0003): `ProjectPort/elder-grace` (this one, the core) and `ProjectPort/grace-eldenring` (the game docs moved there from `docs/games/elden-ring/`, `git init`, its own `CLAUDE.md`). `.local/` stays shared in `ProjectPort/`. A pointer `ProjectPort/CLAUDE.md` (not under git). Links in the core docs rewritten to `../grace-<game>/docs/`.
- Metal Toolchain: after `sudo xcodebuild -runFirstLaunch` (run by the user), `xcodebuild -downloadComponent MetalToolchain`, `xcrun metal` → Apple metal 32023.922.
- GPTK: the user chose Apple GPTK. Installed the cask `gcenx/wine/game-porting-toolkit` 3.0-3 (Wine + D3DMetal; `brew trust gcenx/wine` with the user's consent; the cask removes quarantine and re-signs the app ad hoc).

**How it was checked**
- After the move: `cargo clean && cargo test` green (an old test binary failed because of a path baked in through `CARGO_MANIFEST_DIR` before the move; a rebuild fixes it); `cargo xtask bundle` and `cargo xtask shim` build.

**GPTK and the first oracle launch**
- GPTK installed: `/Applications/Game Porting Toolkit.app`, D3DMetal 3.0, `wine64` (reports "wine-7.7 (Game Porting Toolkit 1.1)"; the version string is old, the build is 3.0-3).
- Prefix: `ProjectPort/.local/prefixes/eldenring` (`wine64 wineboot --init`, ~14 s).
- Added `shim/load-probe` (Windows exe): `wine64 load-probe.exe graphics_proxy.dll` → "shim loaded, protocol 1", exit 0. **The shim loads inside Wine: the bridge to phase 2 is verified.**
- Elden Ring from the sandbox starts but exits: it needs a Windows Steam client in the prefix (details in `grace-eldenring/docs/README.md`, "Oracle"). `steam_appid.txt` (1245620) was put into the sandbox.

**Update:** Windows Steam installed into the GPTK prefix (`.local/downloads/SteamSetup.exe`, Valve's official CDN, sha256 `7d365453…12bcb`), but after self-updating the 64-bit client crashed at startup under GPTK's Wine 7.7 (solved in sessions 0e–0f with the compat shims).

---

## 2026-11-07 — Session 0c: environment and workspace skeleton

**Done**
- Environment: Rust 1.99.1 (`rustup update`), target `x86_64-pc-windows-gnu` + mingw-w64 (GCC 16.2), .NET 10.0.403 (`brew install dotnet`; external tools may need `DOTNET_ROOT=/opt/homebrew/opt/dotnet/libexec`), SteamCMD.
- Cargo workspace: `grace-core`, `grace-proto`, `grace-ctl`, `grace-app`, `xtask`, `shim/graphics-proxy`.

**How it was checked**
- `cargo clippy --all-targets`: no warnings; `cargo test`: 2 tests green (the example config parses, protocol round trip).
- `cargo xtask bundle` → `target/Grace.app`, ad hoc signature. Launch: a window, Metal on "Apple M5 Pro", **120 FPS** (ProMotion), drawable 2560×1600.
- `grace-ctl ping` → `pong: grace-app 0.1.0 (protocol 1)`; `grace-ctl stats` answers from the live window.
- `grace-app --capture .local/captures/phase0-triangle.png --size 1280x800`: the PNG was opened and checked: a gradient triangle on a dark background, as intended.
- `cargo xtask shim` → `graphics_proxy.dll`, a PE32+ x86-64 DLL with the `grace_shim_version` export.
- Screen capture (`screencapture`) from the agent session did not work at the time (no screen recording permission); `--capture` is used to check frames.

---

## 2026-11-07 — Session 0b: a pristine copy of Elden Ring

**Done**
- SteamCMD installed (`brew install --cask steamcmd`), but login hit Steam Guard. Downloaded through the console of the already logged-in Steam client (`download_depot`): depots 1245621, 1245624, 2778580. Depots 1245622/1245623 are not granted for the license (regional, not needed).
- Assembled in `~/Games/EldenRing/Game` (by moving, no copies) and made read-only. The sandbox is an APFS clone in `.local/game_bean/elden_ring`. `config/local.toml` created.
- Checked: 64 files, all `Data*.bhd/.bdt`, DLC, `regulation.bin`, `oo2core_6_win64.dll`; the exe is x86-64, ProductVersion 2.7.2.0; the BHD header is encrypted. Writing into the pristine copy fails with `Permission denied`.

---

## 2026-11-07 — Session 0: concept and documents

**Done**
- Discussed the initial `CLAUDE.md`/`PLAN.md` (an engine only for Elden Ring, no Wine, C# + C++) and rebuilt the concept:
  - the project is a tool for porting any game, Elden Ring is the first test target;
  - the oracle under CrossOver/GPTK + mode C as a proving ground + mode A as the long path (ADR 0001);
  - the stack is Rust + Metal through objc2 (ADR 0002).
- Analyzed Horizon (Minecraft in Rust): took the ideas of resource preparation, differential harnesses, `ctl` + MCP (see `REFERENCES.md`).
- Written: `CLAUDE.md`, `README.md`, `docs/PLAN.md`, `docs/ARCHITECTURE.md`, `docs/REFERENCES.md`, `docs/THIRD_PARTY.md`, ADRs 0001–0002, `docs/games/README.md`, game docs, `.gitignore`, `config/local.example.toml`.

**Environment (measured)**
- macOS 27.3 (26C5035r), Xcode 27.2 (27B52), Apple M5 Pro, 48 GB.
- Rust 1.87.0 / cargo 1.87.0 via rustup 1.28.2 (outdated, updated in session 0c).
- CMake 4.4.3, Homebrew present.
- Missing at the time: Metal Toolchain, .NET SDK, SteamCMD.

**Decisions**
- The game is installed as a pristine copy from the user's Steam library (no other copies are used).
- The config of the friend's Mac (M4) is unknown: to ask.
- IW4L: no link yet: to ask or find.
