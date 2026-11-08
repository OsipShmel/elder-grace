# Elder Grace architecture

## The physics of the problem

Under CrossOver/GPTK a Windows game runs like this:

```
game.exe (x86_64)        ─┐
Wine (x86_64)             ├─ one process, entirely through Rosetta 2
D3DMetal / DXVK (x86_64) ─┘   → Metal
```

macOS does not allow arm64 and x86_64 code in the same process. The whole project rests on two consequences:

1. While the original exe runs, **any** of our code inside its process is translated too. Native speed is possible only in a **separate process** (IPC, shared memory) or in **mode A**, where there is no original exe.
2. Falling back to translation "per subsystem" exists only in mode C. In mode A the fallback is all or nothing: play the original under CrossOver until our port catches up.

## Porting modes

| Mode | What runs | Pros | Cons |
|---|---|---|---|
| **Oracle** | The original under translation | The exact game; a measuring instrument | Not a port |
| **C0** | Original + our shim (observation) | Shows everything the game does with the API | Speeds nothing up |
| **C1** | Original + shader overrides | Fast, measurable GPU wins | Logic and the API layer stay under Rosetta |
| **C2** | Original + rendering in a separate arm64 process | Playable, native rendering | Scene sync, IPC, hard |
| **A** | Only our runtime on top of the game data | Highest ceiling, fully native | Long tail of gameplay parity |
| B | Static recompilation of the exe to arm64 | The exact game | Obfuscation/DRM, Win32, legally grey. **Out of scope** |

C2 is implemented as **C2-b** (ADR 0004): the shim and tools read the *scene state* of the live game (camera, objects, animation, lighting) and our arm64 process renders it from the game's own files.

The same core code (`grace-render`, `grace-ctl`, `grace-shader`, `grace-proto`) works in every mode. Mode C builds the tools and produces numbers before mode A is ready. The oracle stays with us for good as the source of differential tests.

## Layers

```
                 ┌──────────────────────────── oracle (Wine/GPTK, sandbox) ─────────────────────────────┐
                 │  original exe  ──►  shims (graphics API proxies, Rust → Windows x86_64)               │
                 └──────────────┬───────────────────────────────────────┬──────────────────────────────┘
                                │ loopback TCP (grace-proto)            │ shared memory (C2-b, planned)
                                ▼                                       ▼
 pristine install ──►  game data layer (grace-<game>)  ──►  native runtime (arm64)
   (read-only)         archives, models, textures, maps       grace-render ─ Metal, grace-<game> viewer
                                ▲
              grace-ctl (control, stats, memory queries) · grace-bench (frame times, xctrace)
              grace-shader (corpus → translation → overrides) · grace-render::compare (vs oracle)
```

## Layout

Two levels: a shared project folder with separate git repositories in it (ADR 0003, ADR 0005).

```
ProjectPort/                  shared folder, not under git itself
  elder-grace/                this repository: the framework (core crates, tools, shims)
  grace-eldenring/            a game repository: its crates (depend on elder-grace via cargo git) and docs/
  grace-<other-game>/         further games, the same way
  .cargo/config.toml          optional local [patch] pointing game repos at ../elder-grace (not in git)
  .local/                     everything local and game-derived, shared by all repositories
    game_bean/<game>/         sandbox: the game's working copy for the oracle and the shims
    cache/<game>/  captures/  traces/  shaders/  prefixes/  frametimes/  tools/
```

Inside `elder-grace`:

```
crates/                 core, without knowledge of specific games
  grace-core/           library: local config (config/local.toml), shared paths
  grace-proto/          library: control protocol for ctl, runtime and shims (line-delimited JSON)
  grace-render/         library: Metal renderer (meshes, BCn textures, instancing, camera, window, PNG capture, frame comparison)
  grace-shader/         library + CLI: DXBC/DXIL containers, signatures, debug names, corpus summary
  grace-bench/          library + CLI: frame-time reports, Metal System Trace analysis
  grace-ctl/            CLI: talks to the runtime and to the in-game shims
  grace-app/            macOS shell (Grace.app): window, Metal, control server (phase 0 skeleton)
shim/                   Windows x86_64 DLLs/EXEs that run inside Wine
  graphics-proxy/       d3d12.dll proxy: shader dump, live overrides, PSO recipes, memory queries
  dxgi-proxy/           dxgi.dll proxy: frame-time log and live stats
  compat-bcrypt/        bcrypt.dll compat shim (+ IAT patcher, winsock and TCP table fixes)
  compat-realtime/      api-ms-win-core-realtime compat shim
  crash-probe/          AeDebug crash reporter
  load-probe/           DLL load checks
tools/grace-irconv/     DXIL → metallib via Metal Shader Converter (x86_64, under Rosetta)
tools/scripts/          small helpers (window id for screenshots)
xtask/                  workflow tasks: bundle, shim, oracle, shaders, overrides, shader, bench, trace
config/                 local.example.toml (local.toml is in .gitignore)
docs/
```

The **public library surface** that game repositories depend on is `grace-core`, `grace-proto` and `grace-render`. Everything else is tooling run from this checkout (`cargo xtask …`, `grace-ctl`). Crates appear when needed: a planned `grace-prepare` (cache preparation) and `grace-mcp` (MCP server) do not exist yet.

The control socket of the runtime defaults to `$TMPDIR/elder-grace/app.sock` and is overridden by `GRACE_SOCKET` (a Unix socket path is limited to about 104 bytes).

## Key mechanisms

### Preparation (`grace-prepare`, planned)
Modeled on `horizon-prepare`:
- reads only the pristine install, checks files against `prepare-lock.toml` (hashes for a specific game version) and honestly rejects an unknown version;
- writes to `.local/cache/<game>/`; the output version stamp is removed before writing and set at the end, so an interrupted run never counts as done;
- a lock file on the output directory (two runs never write at once);
- `--check` reads no game data and only reports what exists, what is stale and which command fixes it;
- exit codes: 0 done, 1 error or gaps, 2 bad command.

### Shims
- Rust `cdylib`, built for `x86_64-pc-windows-gnu`, put into the sandbox, loaded through a Wine DLL override.
- Modes: pass-through (default) → logging → dump → override.
- Talks to the native side over loopback TCP (Winsock works in Wine).
- Runs under Rosetta, so it is kept thin: heavy work happens on the native side.

### Compat shims for the oracle environment (`shim/compat-*`)
Small proxy DLLs that fix incompatibilities between an old Wine (GPTK = Wine 7.7) and modern Windows programs without touching Wine itself.
- Put next to the exe and enabled per program: `HKCU\Software\Wine\AppDefaults\<exe>\DllOverrides`, value `<dll>=native,builtin`.
- Export every function of the original: naked `jmp` trampolines into the real builtin DLL (loaded by full path from system32), fixing only what is needed.
- `compat-bcrypt` → `bcrypt.dll`: translates algorithm pseudo-handles (`BCRYPT_RNG_ALG_HANDLE` = 0x81 etc.) into real ones. Check: `load-probe.exe bcrypt-rng` (without the shim it crashes, with it status 0). It also hosts the other fixes, because both Steam and `steamwebhelper` load it early:
  - `iat.rs`: a generic import table patcher by name or ordinal, target module or `*` (all), plus patching modules at load time via `LdrRegisterDllNotification`;
  - `getproc_fallback.rs`: when Wine's `GetProcAddress` returns NULL, look the function up in the compat DLL (this is how Chromium gets `QueryUnbiasedInterruptTimePrecise`);
  - `winsock.rs`: `getsockname` for sockets from `AcceptEx` (Wine 7.7 does not know their local address): an `AcceptEx` wrapper handed out via `WSAIoctl` remembers the listening socket;
  - `tcp_owners.rs`: a registry of TCP connections with PIDs in prefix-wide shared memory, appended to `GetExtendedTcpTable` (Wine on macOS returns an empty table).
  - When a second game or host appears, move this into a separate `compat-core` crate.
- `compat-realtime` → `api-ms-win-core-realtime-l1-1-1/2.dll` in `system32`: `Query*InterruptTime*` and others from Windows 10.
- `crash-probe.exe`: an AeDebug debugger instead of `winedbg`: a report with `module+offset`, the stack and strings from memory near the crash (`C:\grace-crash\`).

### The oracle environment in one command (`cargo xtask oracle`)
`setup` (prefix, shims, crash-probe, registry, Windows Steam; idempotent) → `steam --ui` (user login, once per prefix) → `run [game]` (from the sandbox, Metal HUD) · `status` · `kill` · `shot <png>`. What to run, the App ID and the window title come from `config/local.toml` (`exe`, `steam_appid`, `window_title`). Logs go to `.local/logs/`. esync (`WINEESYNC=1`) is on by default; every process of a prefix, wineserver included, must use the same mode.

### The `d3d12.dll` shim (`shim/graphics-proxy`)
- A proxy with Windows ordinals (`D3D12CreateDevice` = 101, `.def` from `build.rs`); other exports are naked trampolines into D3DMetal.
- D3DMetal object vtables live in read-only memory of the macOS framework and Wine's `VirtualProtect` does not change that, so the device object gets a **vtable copy** (128 slots) with our slots: `QueryInterface` (0), `CreateGraphicsPipelineState` (10), `CreateComputePipelineState` (11), `CreatePipelineLibrary` (44), `CreatePipelineState` (47, subobject stream).
- **Never replace D3DMetal resource vtables**: D3DMetal recognizes its resources by them and the game crashes. Vtable copies keep the plain layout (the original vtable address at index −1); copies that also preserve the ABI words before the vtable crashed the game.
- Shaders are written to `GRACE_SHADER_DUMP` (`oracle run` sets `.local/shaders/<game>/`) as `<fnv64>.<stage>.dxbc`, summary in `_stats.txt`.
- **Live overrides** (`live.rs`, `recipes.rs`): a pipeline with an override gets two PSOs, the original (the game receives it) and the override; the pair table is lock-free for readers. Command lists (`CreateCommandList` 12, `CreateCommandList1` 51 on ID3D12Device4) get a vtable copy (96 slots; index −1 holds the original vtable, so several vtable variants may coexist) hooking `QueryInterface` 0, `Reset` 10, `ClearState` 11, `SetPipelineState` 25. Slot numbers are checked against mingw-w64 `d3d12.h`.
- **Reload from disk:** with `GRACE_SHADER_OVERRIDE` set, every PSO (device and library) gets a recipe: a deep copy of its description without bytecode (stage hashes; input layout, stream output, view instancing with their strings; CachedPSO zeroed), about 1 KB. The PSO and its root signature get an extra reference, so the address is never reused and a pair can be set at any time. `reload` re-reads the overrides and recreates the overridden PSOs on the game's device: original bytecode comes from the dump, new bytecode from the overrides. Old override PSOs are never freed (they may be in flight on the GPU).
- Control: TCP `127.0.0.1:47322` (`GRACE_D3D12_PORT`), `grace-ctl overrides [status|on|off|toggle|reload]`, `cargo xtask overrides --reload`.
- **What draws the frame:** `grace-ctl bound [--ms 1000] [--ps] [--top N]`: for the recording window the shim records PSO bindings in command lists and aggregates them by recipe shaders (hash.stage, bind count, DXIL/DXBC). This is how override targets are chosen.
- **Game memory queries** (`resources.rs`): `grace-ctl camera --layout <json>`, `peek <addr>`, `find-floats min:max …`. The shim walks writable memory (`VirtualQuery`) and reads it with `ReadProcessMemory`, so a vanished region is an error rather than a crash (~7 GB in ~12 s). The camera is found by a self-validating signature (`grace_proto::camera_at`: the view-to-world translation equals the position, the rotation is orthonormal, the projection is perspective); a second pass finds matrices with the same rotation and a different translation (world-space copies). Layouts are game knowledge and come with the request.

### Shader overrides: source kinds (`xtask/src/overrides.rs`)
A file `../grace-<game>/shaders/overrides/<hash>.<stage>.<kind>`, where hash is the fnv64 of the original from the dump:
- `.hlsl`: a full replacement: DXBC original → vkd3d `<stage>_5_0`, DXIL → dxc with the original's model.
- `.irpatch`: a patch of the **original DXIL without sources**: `dxc -dumpbin` → operations on `dx.op.storeOutput.f32` (`output <id> mul|add x [y z w]`) → `dxa` → `dxv`. Only the patch is in git; the generated `.ll` lives in `.local/shaders/<game>-override-src/` (disassembled game code is game data, rule 3).
- `.local/shaders/<game>-override-ll/<hash>.<stage>.ll`: free-form manual IR edits, local only.
- After the build, input/output signatures are compared with the original (warning on mismatch); overrides without a source are removed from `.local/shaders/<game>-override/`.
- Verified: the game's disassembled DXIL reassembles with `dxa` and passes `dxv`; the container only loses `RTS0` (the root signature comes from the PSO description) and `ILDN`.
- Info: `cargo xtask shader <hash>.<stage> [--ir]` (name, numbered signatures, resources, disassembly); search by name with `grace-shader names <dump> <substring>`.

### The `dxgi.dll` shim (`shim/dxgi-proxy`) and measurements
- A dxgi proxy (GPTK ordinals, `.def` from `build.rs`). Vtable copies of the factory (`QueryInterface` 0, `CreateSwapChain` 10, `CreateSwapChainForHwnd` 15) and the swapchain (`QueryInterface` 0, `Present` 8, `Present1` 22). `Present` with `DXGI_PRESENT_TEST` is not counted.
- Log `GRACE_FRAMETIME_LOG` → CSV `frame,t_ms,dt_ms`; `oracle run --label L` writes `.local/frametimes/<game>/L.csv`. `--no-shim` removes only the d3d12 proxy, so frame time is measured the same way in both modes.
- Live stats: TCP `127.0.0.1:47321` (`GRACE_SHIM_PORT`), `grace-proto`; `grace-ctl --shim stats|watch`.
- `cargo xtask bench <label> [<label2>]` (`crates/grace-bench`): average FPS, p50/p95/p99, 1% and 0.1% low, stutters (> max(2×median, 25 ms)), histogram, B vs A comparison.
- `cargo xtask trace [--seconds N] [--label L] [--template T]`: Metal System Trace (`xctrace record --attach`) of the running game → `.local/traces/<game>/L.trace` → analysis: GPU occupancy (union of top-level `metal-gpu-intervals`), per-thread CPU load (`ThreadCPUUsage`), verdict GPU-bound / CPU-bound / waiting. `--template "System Trace"` records thread states (waits).

### Shader pipeline (`grace-shader`, `tools/grace-irconv`): `cargo xtask shaders [game]`
1. `grace-shader inventory`: parses DXBC containers (DXBC SM4/5 and DXIL SM6), summary by stage and model.
2. `grace-irconv` (x86_64, under Rosetta): DXIL → `.metallib` via Apple Metal Shader Converter (`libmetalirconverter.dylib` from D3DMetal in GPTK, x86_64 only; used locally). Output: `.local/shaders/<game>-metal/`.
3. `grace-shader metallib`: native (arm64) load of every `metallib` into an `MTLDevice` and creation of its functions.
4. DXBC SM4/5 → SPIR-V (`vkd3d-compiler` 1.19, built from source into `.local/tools/bin`) → MSL (SPIRV-Cross `--msl-version 30000`) → `xcrun metal` + `metallib`. Intermediate `.spv`/`.metal`/`.air` go to `.local/shaders/<game>-msl/`.
5. Building vkd3d (once): sources from dl.winehq.org → `./configure --prefix=<ProjectPort>/.local/tools --disable-tests --disable-doxygen-doc BISON=/opt/homebrew/opt/bison/bin/bison LEX=/opt/homebrew/opt/flex/bin/flex CPPFLAGS=-I/opt/homebrew/include LDFLAGS=-L/opt/homebrew/lib` → a full `make` (the `make vkd3d-compiler` target skips header generation) → `make install`. Needs from Homebrew: `bison`, `flex`, `spirv-headers`, `vulkan-headers`, `molten-vk` (for `configure` only), `spirv-cross`.
6. Building dxc (once, ~10 min on an M5 Pro): `git clone --depth 1 --branch v1.9.2609 --recurse-submodules --shallow-submodules https://github.com/microsoft/DirectXShaderCompiler.git .local/src/dxc` → `cmake -S . -B build -G Ninja -C cmake/caches/PredefinedParams.cmake -DCMAKE_BUILD_TYPE=Release -DHLSL_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_TESTS=OFF -DCLANG_INCLUDE_TESTS=OFF -DSPIRV_BUILD_TESTS=OFF` → `ninja -C build dxc dxa dxv` → copy `build/bin/{dxc,dxa,dxv}-3.7` to `.local/tools/bin/{dxc,dxa,dxv}` and `build/lib/libdxcompiler.dylib` to `.local/tools/lib/`. Needs `cmake`, `ninja` (Homebrew). The built-in validator signs DXIL (the container hash is non-zero), so `dxil.dll` is not needed.
7. For now this is a **compilability check**: resource bindings (D3D12 root signature → Metal arguments) are not mapped, so these shaders do not draw yet. Game shaders already run natively on the GPU through D3DMetal; translated shaders matter for our own renderer (C2-b, mode A), not for injection into the game process.

### Native renderer (`grace-render`)
- Input: `Scene { meshes, textures, instances }` in world space; meshes carry positions, normals, UVs, indices and an optional albedo texture index. Output: a window (`viewer::run`, orbit/fly camera) or a PNG (`Gpu::capture_png`).
- Metal through `objc2-metal`; Shared buffers (unified memory, no blit copies), memoryless depth, BCn textures without transcoding, instancing (matrices in buffer 4), alpha test.
- `compare::side_by_side`: our frame next to the oracle's, the oracle frame center-cropped to our aspect ratio (window bars).
- Game repositories turn their formats into `Scene` (for Elden Ring: `grace-eldenring/crates/er-viewer`).

### Control (`grace-ctl`, `grace-mcp` planned)
- One JSON protocol over a Unix socket (runtime) or loopback TCP (shims: dxgi 47321 for stats; d3d12 47322 for overrides and memory queries).
- Current commands: `ping`, `stats`, `watch`, `overrides`, `bound`, `camera`, `peek`, `find-floats`. Planned: `camera set/get` on the runtime, `feature on/off`, `capture`, `wait-for <event>`, `events` (no polling).
- An MCP server will give an agent the same commands directly.

### Comparison with the oracle
- The camera comes from the live game (shim memory queries), the same camera is set in our renderer, both frames are captured and joined side by side; a numeric diff metric is planned.
- Performance: one route, one preset, one resolution; numbers go to `../grace-<game>/docs/BASELINE.md` and `PROGRESS.md`.

## Target hardware

- Development: M5 Pro, 20 GPU cores, 48 GB.
- Minimum: M4 (exact config of a friend's Mac to be confirmed), assume 16 GB unified memory.
- Metal features only through device capability checks. Memory and streaming budgets come from the minimum configuration.
