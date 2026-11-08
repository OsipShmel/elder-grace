# 0005: Elder Grace as a standalone framework; ports pull it in as a dependency

- Date: 2026-11-08
- Status: **accepted**; mechanism: **cargo git dependency + local `[patch]`** (user's decision, 2026-11-08)

## Context

Until now (ADR 0003) the two repositories sat side by side in `ProjectPort/` and were linked by relative paths (`../elder-grace/crates/...`). Cross-cutting changes went in two branches with the same name (`feat/game-camera`), and the game's `main` could reference unmerged engine code. Elder Grace is meant as a framework for porting games (Elden Ring is the first proving ground), so it has to be shaped as a versioned library with its own remote.

## Decision

1. `elder-grace` has its own remote (`https://github.com/OsipShmel/elder-grace`, public) and is shaped as a framework: public `grace-*` library crates with clear boundaries and versions (`vX.Y.Z` tags), a README ("what it is and how to depend on it"), a CHANGELOG.
2. `grace-eldenring` has its own remote (`https://github.com/OsipShmel/grace-ER`, public; checked out locally as `grace-eldenring/`) and consumes the framework: a README ("how to build the port"), dependencies on engine versions.
3. **Language:** documentation, documents for AI agents (`CLAUDE.md` and others), code comments and messages (errors, logs, CLI), and commit messages are **in English**. Conversation with the user stays in Russian.

## How the engine is pulled in: cargo git + `[patch]`

| Option | Pros | Cons |
|---|---|---|
| **git submodule** `grace-eldenring/engine → elder-grace` + path dependencies | exact engine version in every port commit; engine code visible inside the port repo | `git submodule update` after clone/checkout; detached HEAD; a cross-cutting change is 2 commits plus a pointer update |
| **cargo git dependency** `grace-render = { git = "…", tag = "v0.1.0" }` + local `[patch]` to a sibling checkout | idiomatic Rust; the version is pinned in `Cargo.lock`; no manual pointer sync | editing the engine alongside the port needs a `[patch]` (a few lines of local cargo config, not in git) |

Decision: **cargo git dependency** on a pinned engine tag (`Cargo.lock` pins the commit). For joint work on the engine and the port, a local `[patch."https://github.com/OsipShmel/elder-grace"]` pointing at the sibling checkout goes into `ProjectPort/.cargo/config.toml` (outside both repositories) or the port's untracked `.cargo/config.toml`. No submodule.

Note: cargo still fetches the git source once even when it is patched, so the engine remote and its tag must exist before a clean build of the port works.

## Consequences

- External engine tooling (`xtask`, shims, `tools/grace-irconv`) stays in `elder-grace`; game data and game knowledge live only in `grace-eldenring` (the core rule does not change).
- `ProjectPort/.local/` (data, prefix, caches) stays shared and outside git; paths to it come from `config/local.toml`.
- The work plan is "Phase R" in `PLAN.md`, done before C2-b (ADR 0004).
