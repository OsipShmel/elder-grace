# 0002: Stack: Rust + Metal through objc2

- Status: accepted (revisit after phase 0 if the bindings get in the way)
- Date: 2026-11-07

## Context

The initial proposal was C# (importer) + C++20/metal-cpp (runtime) + CMake/Xcode. What changed:
- An agent writes almost all the code. Memory errors and UB in agent-written C++ are the most expensive kind: they are silent and surface days later.
- The project is now a tool for different games, plus a Windows x86_64 shim inside Wine.
- The Horizon example (Rust, about 40 crates, agent-driven development) shows Rust carries a project of this size.

## Decision

- **Rust** (edition 2024) for the core, runtime, parsers, tools and shims. A cargo workspace.
- **Metal, MetalFX, AppKit, GameController** through the `objc2-*` crates. Shaders in MSL.
- **Shims**: `cdylib`, cross-compiled for `x86_64-pc-windows-gnu` (mingw) or `-msvc` (`cargo-xwin`); chosen in phase 0 by what loads better in Wine (it is `-gnu`).
- **Swift**: optional for a launcher/settings (SwiftUI), through the C ABI, never in the hot path.
- **C/C++**: selectively through FFI (for example, Jolt) when there is no acceptable Rust alternative.
- **C# / .NET**: only as a reference for differential tests (SoulsFormats etc.), not in the product.

## Alternatives

- **C++20 + metal-cpp.** More gamedev examples, direct access to every C++ library. Downside: UB and memory errors in agent code, a two-language build (C# + C++).
- **All Swift.** The best Apple integration. Downsides: ARC and less control in the hot path, a weak ecosystem for binary parsing and reverse engineering, a Windows shim in Swift is impractical.
- **wgpu instead of Metal directly.** We do not need cross-platform support, and we need MetalFX, tile memory and memoryless targets directly.

## Consequences

- Rust had to be updated: the machine had 1.87, edition 2024 and current `objc2-*` need a recent stable.
- Parity knowledge from SoulsFormats is carried over as documentation and checked with differential tests, not copied as code (unless the license allows otherwise).
- The `.app` bundle (Info.plist, icon, signing for local runs) is built by our own `xtask`.
