# 0006: Live game state through a per-frame memory mirror

- Date: 2026-11-08
- Status: **accepted** (design approved by the user, 2026-11-08)

## Context

C2-b (ADR 0004) renders the live game's scene state in our native process. The state lives in the game process under Wine/Rosetta. The existing control channel (JSON over TCP, memory walks) is fine for discovery but too slow and too chatty for every frame.

## Decision

1. The game-side shim (`dxgi-proxy`, at `Present`) copies a set of memory regions into a fixed 64 KiB file-backed record; the native process maps the same file. A seqlock in the header gives consistent snapshots without locks or syscalls on the read side.
2. The region set is chosen by the game repository and sent over the existing control protocol (`Request::Mirror`). The engine never knows what the bytes mean.
3. The host side is a generic `LiveSession` in the new `grace-live` crate; each game implements a `LiveAdapter` (discover regions, decode a snapshot into engine types).

Alternatives considered: UDP datagrams per frame (simple, but push-only and needs a receive loop; no advantage for "latest state" reads); querying over TCP per frame (a round trip and JSON per frame).

## Consequences

- One mechanism for B0–B3: camera, objects and bones differ only in the adapter.
- The record format is versioned (`version` in the header); a change bumps it.
- If Wine does not share writable file views with native mmap, the shim falls back to writing the record with `WriteFile` each frame; the reader does not change.
