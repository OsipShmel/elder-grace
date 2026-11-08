# Elder Grace work plan

The agent reads this file at the start of every session and writes progress to `docs/PROGRESS.md`. The plan is alive: when reality diverges from it, update the plan and briefly explain why.

**Goal:** a toolset and native engine for porting Windows games to macOS (Apple Silicon) that gives noticeably more performance and "Mac-ness" than translation (CrossOver/GPTK). We aim for +20–30% over the oracle at the same settings, or better settings at the same FPS.

**How:** through the first test target (Elden Ring, repository `../grace-eldenring/`). Each phase is described generically; what it means for a specific game lives in that game's docs.

**Priority:** measurement → graphics and performance → game data → native runtime → gameplay.

Every phase ends with a **milestone**: something the user runs themselves, with a measurable criterion. Phases 1, 3 and 5 are followed by **forks**: decisions made by numbers, not by faith.

**Status (2026-11-08):** phases 0–2 done; phase 3 done as tooling (the game is CPU-bound, see fork 1); phases 4–5 done as path A (milestones M1–M6, data layer in `grace-eldenring/crates/er-data`); fork 3 decided: **C2-b**. Phase R (restructuring) done. Next: **Phase 6: C2-b**, on the user's go.

---

## Phase 0: Foundation (done)

**Why:** environment, repository skeleton, a legal copy of the first target.

Tasks:
1. **Environment.** Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`), current Rust (`rustup update`), a cross target for the shims (`x86_64-pc-windows-gnu` + mingw), SteamCMD, an oracle environment (GPTK). Versions recorded in `PROGRESS.md`.
2. **First target.** Pick a game (criteria in `docs/games/README.md`). Install a pristine copy from the store into `pristine_dir`, make a sandbox `sandbox_dir`, create `config/local.toml`.
3. **Repository skeleton.** A cargo workspace with crates from `docs/ARCHITECTURE.md`. `grace-app` opens an empty Metal window as an `.app`. `grace-ctl` answers `ping`.
4. **Licenses.** Go through `docs/REFERENCES.md`, fill `docs/THIRD_PARTY.md` for everything we plan to take as code.

**Done when:** `Grace.app` opens, `grace-ctl ping` answers, the pristine copy is read-only, the environment is recorded.

**Milestone:** internal, a short report only.

---

## Phase 1: Oracle and baseline (done)

**Why:** find out where performance is really lost. Without that, any 20–30% is guesswork.

Tasks:
1. Run the game in the oracle environment from the sandbox, offline.
2. **A reproducible route** (or several): start point, path, duration. Described in `../grace-<game>/docs/BASELINE.md` so that both the user and the agent can repeat it.
3. Metal HUD (FPS, frame time), `Metal System Trace` via `xctrace`, per-thread CPU load.
4. **Metal frame capture** of the translated game for 2–3 typical frames: pass count, draw calls, render targets, heaviest shaders.
5. Describe the game's frame structure in `../grace-<game>/docs/FORMATS.md` ("Rendering" section).

**Done when:** `BASELINE.md` has numbers for 2–3 presets and answers **"are we bound by the CPU (translation, driver, logic) or by the GPU (shaders, passes, bandwidth)"**.

**Milestone:** the user runs the route and confirms the numbers match their impression.

**Fork 1:**
- **GPU-bound** → most effort goes into phases 2–3 and native rendering (5).
- **CPU-bound** → phases 2–3 are done minimally (they are still needed as tools), and the main bet is mode A (phases 4–5 and beyond).
- **Result:** CPU-bound (main thread under Rosetta + D3DMetal translation, GPU ~67% busy); esync gave +~20%.

---

## Phase 2: Shim C0: observation from inside (done)

**Why:** see what the game does with the graphics API and connect the oracle to our tools.

Tasks:
1. Graphics API proxy DLLs (for DX12 games `d3d12.dll` / `dxgi.dll`), Rust, cross-built for Windows x86_64, loaded in the sandbox through a Wine DLL override. Transparent pass-through first.
2. Logging: pipeline, shader, resource creation; frame stats.
3. **Shader corpus dump** into `.local/shaders/<game>/` with hashes. Catalog: which shader is in which pass and what it costs (from phase 1 captures).
4. A channel to `grace-ctl` (loopback TCP): stats, switches, "capture a frame".
5. Camera constants (matrices from constant buffers), so our camera can be put at the same spot to compare frames. Done via memory queries (`grace-ctl camera`).

**Done when:** the game runs in the sandbox with the shim losing no more than 2–3% FPS; the shader corpus is collected; `grace-ctl stats` shows live game stats.

**Milestone:** the user plays with the shim and sees the overlay or `grace-ctl` output.

---

## Phase 3: Shim C1: shader overrides and first numbers (done as tooling)

**Why:** the first real results of reworked graphics in the live game.

Tasks:
1. The `grace-shader` pipeline: corpus → readable form (DXIL → MSL / SPIR-V etc.) → edit → back into a format the translation layer accepts.
2. Override mechanism in the shim: substitute our version by shader hash, toggle live through `grace-ctl`.
3. Optimize the 3–5 most expensive shaders from phase 1, measuring each before/after on the route.
4. Evaluate pipeline warm-up and caches (stutters) if they are visible in the oracle.

**Done when:** there is a table "shader → ms/frame gain" and a total gain on the route with traces.

**Milestone:** the user compares the game with and without overrides (live toggle).

**Fork 2:** how much more can mode C give? If the ceiling is close, move to phases 4–5.
- **Result:** the override mechanism works (live toggle, reload, `.hlsl` and `.irpatch`), but the game is CPU-bound, so shader optimization does not pay off; tasks 3–4 are not done.

---

## Phase 4: Data layer (done as path A M1–M2)

**Why:** read the game's data natively. The foundation of mode A and a source of knowledge for mode C.

Tasks:
1. A `grace-prepare` framework: reads the pristine install, checks it against a hash lock file, writes a cache to `.local/cache/` with a version stamp (removed before writing and set afterwards, so an interrupted run never counts), a lock on the directory, a `--check` mode that reads no game data. *Not done yet: the data layer reads the install directly.*
2. A game module: archives, compression, containers, models, textures, materials, maps (the list is in the game docs).
3. Differential tests of the parsers against a community reference tool.
4. The first import target: one small enclosed scene (each game has its own, see its docs). Report: what is recognized, what is not and why.

**Done when:** the test scene cache is ready, every mesh and texture loads, anything unrecognized is listed with an explanation.

---

## Phase 5: Native renderer MVP: "ghost walk" (done as path A M3–M6)

**Why:** see the game world in a native app and compare it with the oracle.

Tasks:
1. A `grace-render` framework: device, queues, frame resources, unified memory without extra copies, BCn textures as is.
2. Static geometry, materials through our own templates, light and fog close to the original (from the phase 1 frame structure and phase 2 shaders).
3. A free camera (keyboard, mouse, gamepad through GameController) and `--capture`.
4. **Comparison with the oracle:** the same camera (from phase 2) → the oracle frame and our frame → side by side (a numeric diff is planned).
5. `Grace.app`: full screen, Cmd+Q, ProMotion, MetalFX, presets.

**Done when:** a flight through the test scene at a stable ≥60 FPS at native resolution on the M5 Pro and acceptable on the minimum config; frames match the oracle in geometry and textures.

**Milestone:** the "ghost walk". The user compares lighting, colors and materials with the original.

**Fork 3:**
- **C2:** move native rendering into a separate arm64 process fed from the live game's shim through shared memory (IOSurface, `MTLSharedEvent`). The playable original game with native rendering.
- **Deep A:** keep growing our own runtime (phase 6+).
- Both use the same `grace-render`. Decided by numbers and by the complexity of syncing scene state.
- **Decided 2026-11-08: C2-b**: native rendering driven by the live game's **scene state** (ADR 0004). Not a full engine replacement: the logic stays original, the graphics are ours.

---

## Phase R: Restructuring (done 2026-11-08)

**Why:** Elder Grace is a framework and a game port is its consumer (ADR 0005). Cross-cutting changes went through same-named branches in two repositories, linked by relative paths.

Tasks (in order):
1. Merge `feat/game-camera` in both repositories. *Done 2026-11-08.*
2. Pull the engine in through a **cargo git dependency** on a version tag + a local `[patch]` to the sibling checkout for joint development (ADR 0005).
3. Remotes: `github.com/OsipShmel/elder-grace` and `github.com/OsipShmel/grace-ER` (public; created and pushed by the user). *Done.*
4. `elder-grace` as a library: public crate boundaries (`grace-core`, `grace-proto`, `grace-render`), versions/tags, README (what it is, architecture, how to depend on it, requirements), CHANGELOG. Game knowledge only in game repositories.
5. `grace-eldenring`: README (what it is, how to build and run the port, what the user must provide: their own copy of the game), a dependency on an engine version.
6. **English:** all docs (including `CLAUDE.md`, ADRs, specs/plans, `FORMATS.md`, `BASELINE.md`), comments, error/log/CLI messages, commit messages from now on. Conversation with the user stays in Russian.
7. Check: a clean clone of both repositories → build, tests, clippy; `er-viewer --map` and `oracle run` work as before.

**Done when:** two independent repositories, the port builds against an engine version, everything is in English, the READMEs are enough for a new person or agent.

---

## Phase 6: C2-b: native rendering driven by the live game

**Why:** play the original game (FromSoftware logic) with Elder Grace's native graphics; then take the game's own rendering away.

Milestones (each checked by automatic comparison with the oracle, as in M6):
- **B0: live camera mirror.** The shim finds the game's camera structure once and every frame publishes the camera and world position through a fast channel (shared memory, not a 7 GB walk). The `er-viewer` window follows the game camera in real time. Numbers: latency, FPS of both windows.
- **B1: the world around the player.** The map by player position: dungeons (`m1x`) and the open world by `m60_XX_YY` tiles with streaming; LOD.
- **B2: dynamic objects.** Characters/enemies/items: which models and where (reading game memory: `ChrIns` etc.), in a rest pose first.
- **B3: animation.** Bone matrices from the game (memory or skinning buffers), skinning in our renderer.
- **B4: materials and lighting as in the game.** Port the game's lighting models to MSL (G-buffer → lighting → tone mapping) from the game's DXIL shaders; exposure, fog, sun from `cbSceneParam` every frame. Normals, SpeedTree trees, bones of static meshes.
- **B5: our frame instead of the game's.** The game does not render (the shim skips its drawing), our window is the main one, input goes to the game. Measure: main-thread CPU and FPS against the oracle.
- **B6: UI.** Decide: the game's UI pass over our frame, or our own UI.

Risks: memory layouts depend on the game version (1.17.2); effects (FXR), particles, UI; synchronizing two processes.

---

## Phase 6+: Depth

The order is decided with the user based on C2-b results. Game specifics live in its docs.
- **World:** streaming, LOD, GPU-driven rendering (culling, indirect), shadows, atmosphere, vegetation, post-processing, Apple GPU specifics (tile memory, memoryless).
- **Character:** skinning, animation, camera, movement, collision (Jolt or Rapier instead of the original physics).
- **Gameplay:** the game's script VMs, params, AI, combat, UI, saves.
- **Audio.**
- **Generalizing the core:** when a second game appears, revise `grace-*` and extract what is shared.

The honest list of what does not work is kept in `../grace-<game>/docs/KNOWN_ISSUES.md`.

---

## Risk register (general)

| Risk | Where | What we do |
|---|---|---|
| The oracle does not start or is unstable in CrossOver/GPTK | Phase 1 | Try both environments; pin versions; change the first target as a last resort |
| The translation layer does not allow shader or proxy DLL overrides | Phases 2–3 | Try different hook points (`dxgi`/`d3d12`, other environments); otherwise use the shim for observation only |
| The shim changes game behavior (timing, synchronization) | Phase 2 | Full pass-through mode; FPS comparison with and without the shim |
| The ceiling of mode C (logic under Rosetta) | Forks 1–3 | Measure early; mode A / C2-b as the main path when CPU-bound |
| Tool licenses do not allow taking code | Phases 0, 4 | Use them as documentation and test references, write our own |
| The native renderer's picture differs from the original | Phase 5, 6 | Compare with the oracle at the same camera |
| Game memory layouts change between versions | Phase 6 | Self-validating signatures instead of fixed addresses; record the game version with every layout |
| The core is generalized "wrong" | Always | The core rule: generalize at the second point of use |
| The project does not fit one session | Always | `PROGRESS.md` as memory; small tasks; frequent commits |
