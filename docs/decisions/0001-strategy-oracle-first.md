# 0001: Strategy: oracle first, mode C as a proving ground, a core grown from real ports

- Status: accepted
- Date: 2026-11-07

## Context

The original plan had a single path: our own engine for Elden Ring on top of the game data (mode A), with no role for Wine/CrossOver at all. The user wants:
- to see results in the foreseeable future (reworked shaders, higher FPS), not only after years of parity work;
- a tool that can port different games, not just one.

Constraints:
- macOS does not mix arm64 and x86_64 in one process. While the original exe runs, everything in its process goes through Rosetta.
- The team has no experience with ports like this, so the risk of drowning in the design of a generic engine is high.

## Decision

1. **Oracle first.** The original game under CrossOver/GPTK is a permanent measuring instrument: baseline, frame capture, differential comparisons. Translation is not how our product runs.
2. **Mode C as a training ground.** A shim inside Wine: observation (C0) → shader overrides (C1) → if needed, moving rendering into a separate native process (C2). It produces numbers quickly and builds tools that mode A needs as well.
3. **Mode A as the long-term path**, with the same core crates.
4. **Forks decided by numbers** after phases 1, 3 and 5 (see `PLAN.md`): CPU-bound or GPU-bound, where the ceiling of C is.
5. **The core is generalized at the second point of use**, not in advance. Game specifics live in a separate game repository `grace-<game>` (see ADR 0003).
6. **Mode B** (static recompilation of the exe) is out of scope.

## Consequences

- Wine/GPTK/CrossOver are needed in the development environment (as the oracle) but are not distributed and are not part of the product.
- A Windows x86_64 shim is needed, hence cross-compilation (see 0002).
- The user's first milestones are the game with a shim and shader overrides, not only a "ghost walk".
- If fork 1 shows the game is CPU-bound, mode C shrinks to tooling and the main bet moves to A.
