# Elder Grace

A framework and toolset for porting Windows games to macOS on Apple Silicon, natively. It works on top of **your own legally purchased copy** of a game: this repository contains no game code or data, and nothing from a game is ever committed.

Translation layers (CrossOver, Wine, GPTK/D3DMetal) run the whole game under Rosetta 2. Elder Grace uses the translated game as a measuring instrument (the *oracle*) and moves the work into native arm64 code step by step:

- **Observe and patch the live game** (mode C): small Rust shims inside Wine dump the game's shaders, swap them live, measure frame times and read scene state from game memory.
- **Render natively** (C2-b, current direction): a native Metal renderer draws the game world from the game's own files, driven by the live game's camera and scene state, while the original game logic keeps running.
- **Reimplement** (mode A, long term): a native runtime that reads the game data without running the original executable.

The first target is Elden Ring, ported in a separate repository: [grace-ER](https://github.com/OsipShmel/grace-ER).

> Status: early research project. APIs change between minor versions. macOS on Apple Silicon only.

## What is in here

| Crate / tool | Kind | What it does |
|---|---|---|
| `grace-core` | library | Local machine config (`config/local.toml`: install, sandbox and cache paths per game), shared paths |
| `grace-proto` | library | Control protocol (line-delimited JSON) between `grace-ctl`, the runtime and the in-game shims; camera signature checks |
| `grace-render` | library | Native Metal renderer: meshes, BCn textures, instancing, orbit/fly camera, window, PNG capture, side-by-side comparison |
| `grace-shader` | library + CLI | DXBC/DXIL container parsing, signatures, debug names, shader corpus summaries, native metallib checks |
| `grace-bench` | library + CLI | Frame-time reports (FPS, percentiles, 1% low, stutters) and Metal System Trace analysis |
| `grace-ctl` | CLI | Talks to the runtime and to the shims: stats, live shader overrides, bound shaders, camera and memory queries |
| `grace-app` | app | macOS shell (`Grace.app`) skeleton: window, Metal, control server |
| `shim/graphics-proxy` | Windows DLL | `d3d12.dll` proxy inside Wine: shader dump, live overrides, PSO recipes, memory queries |
| `shim/dxgi-proxy` | Windows DLL | `dxgi.dll` proxy: frame-time log, live stats |
| `shim/compat-*`, `shim/crash-probe` | Windows DLL/EXE | Fixes that let modern Steam run on GPTK's Wine 7.7; an AeDebug crash reporter |
| `tools/grace-irconv` | CLI (x86_64) | DXIL → metallib through Apple's Metal Shader Converter |
| `xtask` | workflow | `cargo xtask bundle | shim | oracle | shaders | overrides | shader | bench | trace` |

Game repositories depend on the three library crates. Everything else is tooling you run from a checkout of this repository. Architecture, mechanisms and decisions: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/decisions/](docs/decisions/), plan: [docs/PLAN.md](docs/PLAN.md).

## Requirements

- A Mac with Apple Silicon, a recent macOS and Xcode with the Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`).
- Rust stable (edition 2024) via rustup.
- For the shims: `rustup target add x86_64-pc-windows-gnu` and mingw-w64 (`brew install mingw-w64`).
- For the oracle: Apple Game Porting Toolkit 3.0 (`brew install --cask gcenx/wine/game-porting-toolkit`) and your own copy of the game.
- Optional for the shader pipeline: vkd3d, SPIRV-Cross and DirectXShaderCompiler built locally (recipes in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)).

## Quick start

```bash
git clone https://github.com/OsipShmel/elder-grace
cd elder-grace
cargo build
cargo test
cargo xtask shim --release      # Windows shims, needs the mingw target
cargo xtask bundle              # target/Grace.app
```

Local paths go into `config/local.toml` (ignored by git); start from [config/local.example.toml](config/local.example.toml). Large local data (Wine prefixes, game sandboxes, caches, captures) lives in a `.local/` directory next to the repositories and never in git.

## Using Elder Grace in a game port

A port is its own cargo workspace that pins an engine tag:

```toml
[workspace.dependencies]
grace-core   = { git = "https://github.com/OsipShmel/elder-grace", tag = "v0.1.0" }
grace-proto  = { git = "https://github.com/OsipShmel/elder-grace", tag = "v0.1.0" }
grace-render = { git = "https://github.com/OsipShmel/elder-grace", tag = "v0.1.0" }
```

To work on the engine and a port at the same time, check both out side by side and add a local patch that is not committed, for example in `ProjectPort/.cargo/config.toml` (the folder that contains both checkouts) or in the port's `.cargo/config.toml`:

```toml
[patch."https://github.com/OsipShmel/elder-grace"]
grace-core   = { path = "elder-grace/crates/grace-core" }
grace-proto  = { path = "elder-grace/crates/grace-proto" }
grace-render = { path = "elder-grace/crates/grace-render" }
```

Paths in a cargo config are relative to the directory that contains `.cargo/`. Cargo still fetches the git source once even when it is patched. The core rule: engine crates contain no knowledge of any specific game; game formats, memory layouts and game docs live in the game repository.

## The oracle in one command

```bash
cargo xtask oracle setup          # Wine prefix, compat shims, crash reporter, Windows Steam
cargo xtask oracle steam --ui     # you log into Steam yourself, once per prefix
cargo xtask oracle run --label L  # the game from the sandbox, with shims, Metal HUD, frame log
cargo xtask oracle status          # what is running in the prefix
cargo xtask oracle kill            # stop the game and every Steam process in the prefix
cargo xtask oracle shot out.png    # screenshot of the game window
```

All commands: [docs/COMMANDS.md](docs/COMMANDS.md).

What to run comes from `[games.<id>]` in `config/local.toml`. Logins are always entered by you; the tools never handle passwords. Nothing here is meant for online play, and anti-cheat is never bypassed.

## Legal

- Use only games you own. The tools read the files of your installation at run time; game files, extracted assets, shader dumps and keys must never be committed or distributed.
- Proprietary tools (GPTK, D3DMetal, Metal Shader Converter) are used locally and are not part of this repository or its builds.
- Third-party code and tools are listed in [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md).
- Elder Grace is not affiliated with any game publisher or with Apple.

## License

Apache-2.0, see [LICENSE](LICENSE).
