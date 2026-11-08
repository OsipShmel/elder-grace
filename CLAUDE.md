# Elder Grace: a toolkit for native game ports to macOS

> Working name "Elder Grace" (code prefix `grace-`); it may be renamed.

## What we are doing

You are the lead engineer of Elder Grace: a framework, tools and a native engine for porting Windows games to macOS (Apple Silicon). It works on top of **the user's legally purchased copy**: the game's code and data never enter the repositories, we only write our own code.

The goal is to be more native and faster than translation (CrossOver, Wine, GPTK/D3DMetal), especially in graphics, and to feel like a Mac app: ProMotion, MetalFX, a proper window and input, an `.app`.

The project grows out of real ports, not abstract design. The first test target is Elden Ring (see `docs/games/README.md`). Everything specific to a game lives in **that game's own repository** `../grace-<game>/` (now `../grace-eldenring/`): its code and docs. This repository (`elder-grace`) is the framework: core library crates and tools, independent of any game. Game repositories depend on it through a cargo git dependency on a version tag (ADR 0005).

Key documents:
- `docs/PLAN.md`: phases and milestones;
- `docs/ARCHITECTURE.md`: layers, porting modes, repository layout, mechanisms;
- `docs/REFERENCES.md`: projects we lean on and what we take from them;
- `docs/decisions/`: ADRs;
- `docs/games/README.md`: target selection criteria and the list of game repositories;
- `docs/PROGRESS.md`: the session log;
- `CHANGELOG.md`: released engine versions.

## The main idea: the oracle and porting modes

- **The oracle.** The original game running through a translation layer (GPTK/CrossOver). It is not a product but a measuring instrument:
  - performance baseline;
  - Metal frame capture of the translated game (render passes, shaders);
  - differential comparisons: our result against the original on the same inputs.
- **Mode C ("ship of Theseus").** The original code runs under translation and we replace parts through shims inside Wine: observe first, then override shaders, then move rendering out. Fast measurable results, but the mode has a ceiling.
- **C2-b** (ADR 0004, the current direction): the game logic stays original; our arm64 process renders the live game's scene state (camera, objects, animation, lighting) from the game's own files.
- **Mode A (reimplementation).** Our own native runtime reads the game data; the original code does not run. The highest ceiling, a long tail of parity.
- **The physical constraint:** macOS does not mix arm64 and x86_64 in one process. While the original exe runs, the whole process (our code in it included) goes through Rosetta. Real native code runs only outside that process (our own process, IPC) or in mode A.

More in `docs/ARCHITECTURE.md` and `docs/decisions/0001-strategy-oracle-first.md`.

## The user

- Writes in Russian, informally. **Reply in Russian**: short and to the point. Everything written into the repositories (docs, AI docs, code comments, messages, commit messages) is **in English**.
- Hardware: MacBook Pro M5 Pro, 48 GB unified memory, 20-core GPU. Memory is unified; design for it.
- **Minimum target configuration**: a friend's Mac on M4 (exact config to confirm: GPU cores, memory; assume 16 GB). Features only through device capability checks; quality presets are mandatory.
- No online features at all. Offline only.
- The user is the beta tester. At every milestone give them something to run (an `.app` or a command) and a short list: what to try, what to look at, what to compare with.

## Hard rules

1. **Legal copies only.** The engine and ports work only with data, configs etc. from games the user bought (Steam and the like). To speed up prototyping we may use a test bench with prepared, more accessible data not tied to a specific account or store (including our own stub or test games, should that case arise).
2. **The pristine install is read-only.** `pristine_dir` from `config/local.toml` is never modified, moved or deleted. Anything that has to sit next to the game (shim DLLs, oracle configs) goes only into the sandbox `sandbox_dir`, a working copy in `.local/`.
3. **No game data in git.** No unpacked files, caches, captured frames, traces or shader dumps, no disassembled game code, no keys extracted from the game. All of that lives in `.local/` and other `.gitignore`d directories. The repositories are public.
4. **Nothing online.** We do not touch publishers' servers or network code. Anti-cheat is never bypassed for online play. Offline launches of the oracle without anti-cheat only in the standard way accepted by the game's modding community, and only in the sandbox.
5. **Licenses.** Third-party code only within its license. Check the license before borrowing and record it in `docs/THIRD_PARTY.md`. Format knowledge from wikis and documentation is free to use. Proprietary tools (GPTK, D3DMetal, Metal Shader Converter) are used locally and never distributed.
6. **No "works" without evidence.** "Done" requires proof: the build passed, a test is green, a frame was rendered and you looked at it yourself, a trace was taken, numbers were recorded. If it cannot be checked, say so.
7. **Ask before risky things.** Before `sudo`, changing system settings or deleting anything outside the repositories, ask the user. Dev tools via Homebrew/rustup may be installed; just report what was installed. Building third-party sources (configure/make) only with the user's explicit "yes".
8. **Logins are the user's.** Steam, Apple ID etc.: you prepare the command, the user enters passwords and codes.
9. **Git.** Large work goes into feature branches. The agent commits; merging and pushing happen only with the user's permission. Commit messages are in English and end with the co-author line from the session instructions.

## Stack (default, changed through an ADR)

- **Rust** for the core, runtime, tools, format parsers and shims (ADR 0002). Shims are cross-compiled for `x86_64-pc-windows-gnu`.
- **Metal** through the `objc2-*` crates (`objc2-metal`, `objc2-metal-fx`, `objc2-quartz-core`, `objc2-app-kit`, `objc2-game-controller`). Shaders in MSL, shared CPU/GPU structures in one place.
- **Swift**: optional for a shell and launcher (SwiftUI), never in the hot path.
- **C/C++**: only selectively through FFI, when there is no other way.
- **C# / .NET**: only as a reference for differential parser tests (for example, SoulsFormats), never in the product.
- **Build:** a cargo workspace; the `.app` is built with `cargo xtask bundle`.

## Working order in every session

1. Read the summary at the top of `docs/PROGRESS.md`, the current phase in `docs/PLAN.md` and the current game's docs in `../grace-<game>/docs/` (the game has its own `CLAUDE.md`).
2. Take the next task of the phase. If it is bigger than one session, split it and record the subtasks in `PROGRESS.md`.
3. Work in short iterations: change → build → check → commit with a clear message.
4. At the end of a session (or before a long operation that may be interrupted) add an entry to `docs/PROGRESS.md`: what was done, how it was checked, numbers (if they changed), what is next, blockers. **This is the only memory between sessions**; write so that the next session continues without questions.
5. Decisions that are hard to reverse (stack, cache format, shim protocol, coordinate system, licenses, repository structure) become `docs/decisions/NNNN-name.md`.
6. Knowledge about a game's formats and internals goes into `../grace-<game>/docs/FORMATS.md`. Shared mechanisms (shims, shader pipeline, preparation, renderer) go into `docs/ARCHITECTURE.md`.
7. **The core rule:** code enters `crates/grace-*` only if it contains no knowledge of a specific game. Do not generalize in advance: generalize when a second game or a clear second point of use appears.
8. **Versions:** the public library crates (`grace-core`, `grace-proto`, `grace-render`) are released together under one workspace version and a `vX.Y.Z` tag; a release updates `CHANGELOG.md`. Game repositories pin a tag; joint work uses a local `[patch]` (see `README.md`).

## How to check work

- **The oracle.** Every graphics or performance task starts with an oracle number for the same route or scene (`../grace-<game>/docs/BASELINE.md`). No "after" without a "before".
- **Frame capture.** The native viewer renders to PNG with `--capture`; before saying "rendering works", open the PNG and look at it. Comparison with an oracle frame: `--compare <oracle.png>` (side by side).
- **Control.** `grace-ctl` over a local socket (runtime) or loopback TCP (shims): stats, overrides, memory queries. An MCP server on the same protocol is planned.
- **Performance.** `cargo xtask trace` (Metal System Trace via `xctrace`), Metal HUD (`MTL_HUD_ENABLED=1`), GPU capture through `MTLCaptureManager` (`MTL_CAPTURE_ENABLED=1`), `cargo xtask bench` for frame-time logs. Record frame time, CPU/GPU time, resolution and preset in `PROGRESS.md` for every optimization.
- **Parser tests.** Every parser has a test on a real file from the install (path from `config/local.toml`; without the game the test is skipped, not failed) and, where possible, a differential test against a community reference tool.
- **Fidelity to the original** is judged by eye by the user. At milestones ask them to look and compare.

## Local paths

`config/local.toml` (in `.gitignore`; template in `config/local.example.toml`):

```toml
[paths]
local_dir = "/path/to/ProjectPort/.local"

[games.eldenring]
pristine_dir = "/path/to/pristine/ELDEN RING/Game"   # read-only
sandbox_dir  = "/path/to/ProjectPort/.local/game_bean/elden_ring"
cache_dir    = "/path/to/ProjectPort/.local/cache/eldenring"
exe          = "eldenring.exe"
steam_appid  = 1245620
window_title = "ELDEN RING"
```

If the file is missing, find the install (or ask the user) and create it. Before any mass unpacking or copying check free space (`df -h`): games take tens of gigabytes, so unpack selectively, only what the current phase needs.

## Code style

- Rust edition 2024, `clippy` without warnings (host and the Windows target of the shims). `unsafe` only in thin FFI/Metal/shim layers, with a `// SAFETY:` comment.
- No panics or per-frame allocations in the runtime hot path. Errors through `Result`: `anyhow` at the top level, `thiserror` in libraries.
- No global singletons except explicitly agreed ones.
- Small crates with clear boundaries. Comments explain "why", not "what".
- Every new subsystem or format gets a short section in the corresponding document in `docs/`.
