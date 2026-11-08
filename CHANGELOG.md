# Changelog

The public library crates (`grace-core`, `grace-proto`, `grace-render`) share one workspace version; each release is tagged `vX.Y.Z`. Until 1.0 a minor version may break APIs.

## v0.1.0 (2026-11-08)

First tagged version, consumed by `grace-eldenring` through a cargo git dependency.

- `grace-core`: `LocalConfig` (`config/local.toml` discovery via `GRACE_ROOT` or upward search), `GameConfig` with `pristine_dir`, `sandbox_dir`, `cache_dir`, `exe`, `steam_appid`, `window_title`; the runtime control socket path.
- `grace-proto`: line-delimited JSON requests `Ping`, `Stats`, `Overrides`, `Bound`, `Camera`, `Peek`, `FindFloats` and their responses; `camera_at`, `projection_at`, `rotation_match`, `CameraReport::world`.
- `grace-render`: `Scene`/`MeshData`/`Instance`, Metal renderer with BCn textures, instancing, alpha test, memoryless depth; `Camera` (orbit, `looking`, fly); `viewer::run`; `Gpu::capture_png`; `compare::side_by_side`; DDS parsing.
- Tooling (not versioned for consumers): d3d12/dxgi shims with live shader overrides and memory queries, compat shims for GPTK's Wine, `grace-ctl`, `grace-shader`, `grace-bench`, `grace-irconv`, `cargo xtask` oracle/shaders/overrides/bench/trace.
- All docs, comments and messages in English.
- License: Apache-2.0 (changed from MIT by the owner after the first push).
