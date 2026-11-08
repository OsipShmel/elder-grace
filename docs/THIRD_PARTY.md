# Third-party code and tools

Everything taken **as code** (dependencies, forks, ported fragments) goes here, as does everything used as an external development tool. The license is checked against the repository itself before adding.

| Project | Version / commit | License (checked) | How it is used | Where |
|---|---|---|---|---|
| winit | 0.30.13 | Apache-2.0 (cargo metadata, 2026-11-07) | Window, events, input; pulls objc2 0.5 alongside our 0.6 | `grace-app`, `grace-render` |
| objc2, objc2-foundation | 0.6.5 / 0.3.2 | MIT | Objective-C bridge | `grace-app`, `grace-render` |
| objc2-metal, objc2-quartz-core, objc2-app-kit, objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT | Metal, CAMetalLayer, NSView | `grace-app`, `grace-render` |
| png | 0.18.1 | MIT OR Apache-2.0 | Writing `--capture` frames, reading oracle frames | `grace-app`, `grace-render` |
| serde, serde_json, toml | 1.x | MIT OR Apache-2.0 | Protocol, config | `grace-core`, `grace-proto` |
| clap, anyhow, thiserror | 4.x / 1.x / 2.x | MIT OR Apache-2.0 | CLI, errors | everywhere |
| quick-xml | 0.41 | MIT (cargo info, 2026-11-08) | Parsing xctrace XML exports | `grace-bench` |

Dependencies of the game repository `grace-eldenring` (its own workspace, MIT; the engine itself is Apache-2.0):

| Crate | Version | License | Why | Where |
|---|---|---|---|---|
| num-bigint, num-traits | 0.5 / 0.2 | MIT OR Apache-2.0 | RSA (`c^e mod n`) for `.bhd` headers | `er-data` |
| base64 | 0.22 | MIT OR Apache-2.0 | PEM keys from the user's exe | `er-data` |
| aes | 0.9 | MIT OR Apache-2.0 | AES-128-ECB of `.bdt` file ranges | `er-data` |
| oozextract | 0.5.7 | MIT (crates.io; the package has no LICENSE file, it is a port of `ooz`; re-check code provenance before distributing) | Oodle Kraken (DCX/KRAK) without the Windows DLL | `er-data` |
| miniz_oxide | 0.9 | MIT OR Zlib OR Apache-2.0 | DCX/DFLT | `er-data` |
| ruzstd | 0.9 | MIT | DCX/ZSTD | `er-data` |
| fromsoft-formats | 0.17 | **GPL-3.0-or-later** | only as a reference for differential tests (dev-dependency, never in the product); not wired up yet | planned |

RSA keys and any game data are never stored in the repositories: the keys are read from the user's `eldenring.exe` at run time.

Transitive dependencies: see `cargo metadata`; run a license check (`cargo deny` or similar) before the first distributed build.

Shader pipeline tools (local only, never distributed):
- Apple Metal Shader Converter: `libmetalirconverter.dylib` from D3DMetal inside GPTK; `tools/grace-irconv` calls it through `dlopen`, it is not in the repository.
- SPIRV-Cross 1.4.363.0 (Homebrew, Apache-2.0): the DXBC → SPIR-V → MSL branch of `cargo xtask shaders`, called as an external program.
- vkd3d 1.19 (WineHQ, LGPL-2.1): sources in `.local/tools/src` (sha256 `034613605baab8ba84674f8d272cf22b5e86bc6bc03fc5728ef9bce07308baa6`), built locally; `vkd3d-compiler` is called as an external program (not linked into our code).
- DirectXShaderCompiler v1.9.2609 (Microsoft, LLVM Release License (University of Illinois/NCSA), plus submodules DirectX-Headers MIT, SPIRV-Headers/SPIRV-Tools Apache-2.0): sources in `.local/src/dxc` (tag `v1.9.2609`, commit `01b62ad4`), built locally; `dxc`, `dxa`, `dxv` and `libdxcompiler.dylib` in `.local/tools`, called as external programs (not linked into our code). Built with the user's permission on 2026-11-08; Homebrew `ninja` (Apache-2.0).
- Homebrew packages for building vkd3d: bison, flex, spirv-headers, vulkan-headers, molten-vk (Apache-2.0).

Development tools that are never distributed (local only): CrossOver, Apple Game Porting Toolkit (D3DMetal), Apple Metal Shader Converter, Xcode/Instruments. They never enter the repository or builds.
