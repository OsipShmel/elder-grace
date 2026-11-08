# 0004: Fork 3: C2-b, a native renderer driven by scene state

- Date: 2026-11-08
- Status: **accepted** by the user

## Context

After phases 4–5 (path A, subproject 1: archives → textures → models → the Stranded Graveyard scene → comparison with the oracle using the camera from the live game) we are at fork 3 of `PLAN.md`: how to turn the native renderer into something playable.

The user's goal is **not** a full replacement of the FromSoftware engine (AI, animation, physics, scripts: years of work), but a native **graphics** side with the original game logic; and, further out, tooling to move parts of the original binary to native code where possible.

Measurements (`grace-eldenring/docs/BASELINE.md`): the bottleneck is the CPU (the game's main thread under Rosetta + D3D call translation in D3DMetal); the GPU is ~67% busy. The game's shaders already run natively on the GPU (D3DMetal → Metal), so pushing our own MSL into the game process is pointless and fragile.

## C2 options

- **C2-a, command stream:** the shim serializes D3D12 calls to our arm64 process, which executes them on Metal (our own D3DMetal). A 1:1 picture for free and generic for any D3D12 game; but it is a full D3D12 implementation plus moving gigabytes of resources between processes.
- **C2-b, scene state:** the shim and tools extract the **meaning** from the live game (camera, objects and their transforms, animation, lights, time of day, effects); our process loads meshes/textures/materials from the game files itself (the data layer) and draws them with its own renderer.

## Decision

**C2-b.** The renderer is entirely ours: it can be faster and better than the original (MetalFX, ProMotion, our own lighting/upscaling), resources are not shuttled between processes, and everything already built fits directly: the data layer (`er-data`), the MSB scene, `grace-render`, the camera from the live game, automatic comparison with the oracle (M6). A deferred win: once our renderer draws the frame, the game can be stripped of its rendering (its draw work is not executed), removing D3DMetal's work and part of the main thread's load.

## Consequences

- Dynamic state (characters, poses, effects, UI) requires reading game memory. That is knowledge about Elden Ring and lives in `grace-eldenring`; the core provides generic primitives (structure search by self-validating signatures, `FindFloats`, `Peek`, shim ↔ runtime channels).
- The game version matters (memory layouts): currently 1.17.2 (app) / 1.17.3 (calibrations).
- The UI (HUD/menus) is a separate decision: let the game's UI pass draw over our frame, or redo it.
- Moving parts of the binary to native code is a distant goal. The physics constraint (arm64 and x86_64 do not mix in a process) leaves either "whole subsystems in our native process" or a static recompilation of the whole exe; our C2-b process becomes their host.
- C2-b milestones are in `PLAN.md` ("Phase 6: C2-b"), preceded by the repository restructuring (ADR 0005, "Phase R").
