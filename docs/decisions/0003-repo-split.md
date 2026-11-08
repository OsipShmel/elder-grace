# 0003: Separate repositories: engine and games

- Status: accepted; the dependency mechanism is superseded by ADR 0005 (cargo git dependency)
- Date: 2026-11-07

## Context

Elder Grace is meant to be a tool for different games. The user wants the build of a specific game (what we test) separated from the engine build. Mode C and mode A use the same core (protocol, `grace-ctl`, frame capture, renderer), so the split cannot follow the modes, only the "core / game" line.

## Decision

- `ProjectPort/` is a shared folder, not under git. It contains:
  - `elder-grace/`: core, tools, shims, shared docs (plan, architecture, ADRs, log);
  - `grace-<game>/`: one repository per game: its crates and `docs/` (README, FORMATS, BASELINE, KNOWN_ISSUES, its own `CLAUDE.md`);
  - `.local/`: shared by all: sandboxes, caches, captures, Wine prefixes.
- Game crates depended on the core by path: `grace-core = { path = "../elder-grace/crates/grace-core" }`. ADR 0005 replaced this with a cargo git dependency on a tagged engine version plus a local `[patch]`.
- `config/local.toml` lives in `elder-grace/config/` for now and describes every game; `grace-core` finds it by walking up the tree or through `GRACE_ROOT`.

## Consequences

- A core change a game needs is two commits in two repositories. Order: core first, then the game.
- An agent session opens in `ProjectPort/` and sees both repositories; a short pointer `CLAUDE.md` sits at the root (not under git).
- The core rule from `CLAUDE.md` becomes physical: game code simply cannot end up in `crates/grace-*`.
