# References

Which projects we lean on and what exactly we take from them. References for specific games live in the game repositories (`../grace-<game>/docs/`).

Before taking any **code**, the license is checked against the repository itself and recorded in `THIRD_PARTY.md`. Licenses below are from memory and marked "verify" unless checked.

## Model projects ("our own client on top of a legal copy")

### Horizon: Minecraft: Java Edition in Rust
- https://github.com/horizon-rust/horizon · MIT (per the README, checked 2026-11-07).
- Rewritten from scratch in Rust (about 40 crates, wgpu + WGSL, runs on Metal on a Mac). Contains no Mojang code or assets.
- Prepares resources from the user's **own copy**, checking it against pinned checksums.
- Clearly written by agents: the repository has `CLAUDE.md`, `AGENTS.md`, `.claude/skills/vanilla-parity/`.
- **What we take:**
  - a separate preparation tool with a hash lock file, `--check`, version stamps and a lock file on the directory (→ `grace-prepare`);
  - differential harnesses: the reference and our code get the same inputs, outputs are compared down to float bits (→ comparison with the oracle);
  - a method-level parity ledger (`tools/port_ledger.py`) and the honest wording "100% of what the harnesses compare";
  - agent control: a local socket, `horizon-ctl` and an MCP server with shared JSON commands, `wait-for`/`events` without polling (→ `grace-ctl`/`grace-mcp`);
  - a fixed simulation tick separate from rendering;
  - dependency patches via `[patch.crates-io]` with a comment on when the patch can go.
- **Why our case is harder:** Minecraft has official Mojang mappings and decompilable Java bytecode, so Horizon ports method by method. Native Windows games (C++, often obfuscated) have no such source, so our reference is the game running under translation, not source code.

### IW4L: Modern Warfare 2 (2009) in Rust
- Mentioned by the user as a reference. Link and details not collected yet. **TODO:** find the repository and analyze it like Horizon.
- Expected trait: the IW4 engine has years of community reverse engineering behind it (IW4x and others), so its knowledge base is much richer than most games'.

### OpenMW: Morrowind
- https://openmw.org · GPL-3.0 (verify).
- The classic mode A: an own engine on top of the data of a purchased game. About 15 years to parity, an honest estimate of mode A's scale.

### Xash3D FWGS: the GoldSrc engine (Half-Life 1)
- https://github.com/FWGS/xash3d-fwgs · license to verify.
- An open engine reimplementation running on modern platforms. Proof that mode A already exists for older games.

## Translation layers (oracle)

| Project | Role | License |
|---|---|---|
| CrossOver (CodeWeavers) | The user's main oracle environment at first | Commercial |
| Apple Game Porting Toolkit (D3DMetal, Metal HUD) | The current oracle environment, DX12 → Metal | Proprietary, local only |
| Wine | Base of both environments, DLL overrides for the shims | LGPL-2.1 (verify) |
| DXVK | D3D9–11 → Vulkan, a source of translation knowledge | zlib (verify) |
| vkd3d-proton | D3D12 → Vulkan, a source of D3D12 knowledge | LGPL-2.1 (verify) |
| MoltenVK | Vulkan → Metal | Apache-2.0 (verify) |

## Shaders

| Project | Why | License |
|---|---|---|
| Apple Metal Shader Converter | DXIL → Metal IR/`metallib` | Proprietary (Apple), local only |
| DirectXShaderCompiler (dxc) | DXIL parsing, disassembly, reassembly | NCSA/LLVM (checked, see THIRD_PARTY) |
| dxil-spirv (from vkd3d-proton) | DXIL → SPIR-V | MIT (verify) |
| SPIRV-Cross | SPIR-V → MSL (readable) | Apache-2.0 |
| naga (wgpu) | Shader translation in Rust | MIT/Apache-2.0 (verify) |

## Rust and the platform

| Project | Why | License |
|---|---|---|
| objc2 and framework crates (`objc2-metal`, `objc2-metal-fx`, `objc2-quartz-core`, `objc2-app-kit`, `objc2-game-controller`) | Metal, MetalFX, AppKit, GameController from Rust | MIT / Zlib / Apache-2.0 (checked, see THIRD_PARTY) |
| wgpu | An example of cross-platform rendering in Rust, its Metal backend as a sample | MIT/Apache-2.0 (verify) |
| mingw-w64 | Cross-building the shims for Windows | verify |

## Physics and the rest (later)

| Project | Why | License |
|---|---|---|
| Jolt Physics | Physics (C++, through FFI) | MIT (verify) |
| Rapier | Physics in pure Rust | Apache-2.0 (verify) |

## Mode B, out of scope (for understanding)

- N64Recomp, XenonRecomp / UnleashedRecomp: static recompilation of console binaries. Works well on fixed hardware and older binaries. For modern PC executables with obfuscation and DRM it is out of reach and legally grey, so we do not do it.
