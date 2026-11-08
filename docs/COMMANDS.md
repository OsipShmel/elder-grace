# Commands

Run from the `elder-grace/` checkout. `[game]` defaults to `GRACE_GAME` or the only `[games.*]` entry in `config/local.toml`.

## The oracle (the original game under GPTK)

| Command | What it does |
|---|---|
| `cargo xtask oracle setup [game]` | Wine prefix, compat shims, Windows Steam. Safe to rerun (after Steam updates). |
| `cargo xtask oracle steam --ui` | Steam with its window, for the first login (you enter the password). |
| `cargo xtask oracle steam` | Steam in the background. |
| `cargo xtask oracle run [game]` | Start the game from the sandbox with the shims, Metal HUD, frame log and live mirror. Starts Steam if needed. Flags: `--no-hud`, `--no-shim`, `--no-overrides`, `--no-esync`, `--label L`. |
| `cargo xtask oracle status` | Wine processes, Steam login state, crash reports. |
| `cargo xtask oracle kill` | **Stop everything in the prefix**: the game, every Steam process, wineserver. |
| `cargo xtask oracle shot out.png` | Screenshot of the game window (works when it is not in front). |

If `oracle kill` leaves something behind (check with `oracle status`), the last resort is `pkill -f wine` (kills every Wine process on the Mac, not only this prefix).

## Shims and shaders

| Command | What it does |
|---|---|
| `cargo xtask shim --release` | Build the in-game shims (`oracle run` installs them into the sandbox). |
| `cargo xtask shaders [game]` | Shader corpus report: DXIL → metallib → Metal check. |
| `cargo xtask overrides [game] [--reload]` | Build shader overrides from `../grace-<game>/shaders/overrides/`; `--reload` applies them to the running game. |
| `cargo xtask shader <hash>.<stage> [--ir]` | Info on one dumped shader; `--ir` disassembles it. |

## Measurements

| Command | What it does |
|---|---|
| `cargo xtask bench <label> [<label2>]` | Frame-time report from a run's log (`--label` of `oracle run`), or A/B of two runs. |
| `cargo xtask trace [--seconds N]` | Metal System Trace of the running game + CPU/GPU verdict; `--template "System Trace"` for thread waits. |

## Talking to the running game (`grace-ctl`)

`cargo run -p grace-ctl -- <command>`; `--shim` talks to the dxgi shim (port 47321), memory and override commands go to the d3d12 shim (47322) on their own.

| Command | What it does |
|---|---|
| `--shim ping` / `--shim stats` / `--shim watch` | Is the shim alive; frame stats; stats every second. |
| `overrides on\|off\|toggle\|status\|reload` | Shader overrides in the running game. |
| `bound` | Which shaders are drawing right now. |
| `camera --layout <json>` | The cbuffer camera of the running game. |
| `peek <addr>` / `find-floats <min:max>…` | Debug: read game memory / find float values. |

## Native app

| Command | What it does |
|---|---|
| `cargo xtask bundle` | Build `target/Grace.app`. |
