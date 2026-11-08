# Porting targets

Every game is a **separate git repository** `grace-<game>` next to this one (ADR 0003, ADR 0005), with a `docs/` directory:
- `README.md`: what the target is, installation, test scenes, how the plan's phases map onto this game, specific risks;
- `FORMATS.md`: knowledge about formats and internals (rendering, archives, scripts, memory layouts);
- `BASELINE.md`: oracle routes and numbers (from phase 1);
- `KNOWN_ISSUES.md`: what does not work (once there is something to run).

The game's code lives there too: formats, preparation, runtime. Its crates depend on `elder-grace` through a cargo git dependency on a tagged version (see the engine README). The core (`crates/grace-*`) knows nothing about games.

## Criteria for choosing a test target

1. **Legally purchased by the user** and interesting to them (they are the beta tester).
2. **Graphics API.** The most valuable for the tool is DX12, then DX11: most games worth porting use them today. OpenGL and Vulkan games often already have native or open paths.
3. **No existing native solution.** If there is a native Mac port or a mature open reimplementation, the value for us is low.
4. **Community knowledge** of the formats (modding, wikis, tools): shortens phase 4 a lot.
5. **Protection.** Offline launch without anti-cheat in the way the modding community does it; exe obfuscation only hurts deep hooks, the graphics shim barely depends on it.
6. **Runs in the oracle** (CrossOver/GPTK) and gives a stable benchmark route.
7. **Size.** Smaller means faster iterations, but at least one target must be "heavy", otherwise no win is visible.

## Candidates

| Game | API | Status | Comment |
|---|---|---|---|
| **Elden Ring** | DX12 | **First target** | Heavy, interesting to the user, a huge modding knowledge base, has an oracle (~40 FPS on medium under GPTK). Downside: scale. Repository `../grace-eldenring/` |
| Half-Life (2023, Steam) | OpenGL / software | Deferred | 32-bit GoldSrc, an open reimplementation (Xash3D FWGS) already exists. Gives little for the DX12 pipeline. Check whether the current Steam version has a native Mac build |
| *Second target: TODO* | DX11/DX12 | Searching | A small DX11/DX12 game from the user's library without a Mac version: proves the core generalizes |

The choice of a target is recorded in `PROGRESS.md`; changing it needs an ADR.
