# API Structure Pass

This document tracks the crate-structure refactor for `apps/ade-api`.

For each area:

- check the current layout
- compare it with the standard Rust and Axum patterns
- identify the structural gap
- implement the simplest consistent layout

## Sources

- Rust Book, modules and file layout:
  - <https://doc.rust-lang.org/book/ch07-05-separating-modules-into-different-files.html>
- Cargo package layout:
  - <https://doc.rust-lang.org/cargo/guide/project-layout.html>
- Axum router composition:
  - <https://docs.rs/axum/latest/axum/struct.Router.html>
- Rust API Guidelines naming:
  - <https://rust-lang.github.io/api-guidelines/naming.html>

## Area 1: Module Root Style

Research:

- The Rust Book describes the current idiomatic file layout as `src/foo.rs` for a module root and `src/foo/bar.rs` for child modules.
- It notes that the older `mod.rs` style is still supported, but mixing styles in one project is confusing.
- Cargo also calls for snake_case module names.

Comparison:

- `apps/ade-api/src` mixed:
  - top-level root files with child folders
  - a separate `routes/` folder plus `router.rs` plus `api_docs.rs`
  - domain modules whose names did not match the file tree cleanly

Gap:

- The crate did not have one obvious organizing convention.
- A reader could not infer the module tree from the filesystem.

Implemented:

- Standardized on the current Rust style:
  - `src/foo.rs` as the module root
  - `src/foo/bar.rs` for child modules
- Removed the free-floating `routes/` folder as a second organizing axis.
- Introduced explicit roots for:
  - `src/api/**`
  - `src/runs/**`
  - `src/session/**`
  - `src/terminal/**`

## Area 2: Transport Boundary

Research:

- Axum’s `Router::nest` and `Router::merge` APIs are built around composing smaller routers into a larger tree.

Comparison:

- The crate spread transport code across:
  - `router.rs`
  - `api_docs.rs`
  - `routes/*.rs`

Gap:

- The HTTP transport layer was not grouped together.
- Public and internal routes were separated from the router, but not by a clear parent module.

Implemented:

- Created one `api` module that owns:
  - router construction
  - OpenAPI registration
  - public handlers
  - internal handlers
- Public handlers now live under:
  - `src/api/public/runs.rs`
  - `src/api/public/uploads.rs`
  - `src/api/public/terminal.rs`
  - `src/api/public/system.rs`
- Internal handlers now live under:
  - `src/api/internal/run_bridge.rs`
  - `src/api/internal/terminal_bridge.rs`
  - `src/api/internal/artifacts.rs`
- `src/api/router.rs` is now the single router composition point.

## Area 3: Domain Ownership

Research:

- Rust projects stay easier to navigate when types live with the domain they describe.
- Naming should follow stable Rust conventions and consistent word order.

Comparison:

- Run-specific request and validation types lived in `session`.
- Run storage lived in `run_store.rs` at the crate root instead of under the run domain.
- `runs.rs` and `terminal.rs` were large and mixed protocol, bridge, template, state, and service logic.

Gap:

- Several types lived in the wrong domain.
- Large files hid the real boundaries inside the module.

Implemented:

- Moved run-owned types and persistence under `runs`.
- Added `src/scope.rs` for the shared workspace/config scope instead of burying `Scope` inside `session`.
- Moved run models to `src/runs/models.rs`.
- Moved run persistence to `src/runs/store.rs`.
- Removed `src/routes/session.rs` because it no longer represented a real public boundary.

## Area 4: File Naming

Research:

- Rust naming guidance prefers stable, unsurprising snake_case module names.
- File names should describe the module, not mirror transport quirks like URL plurals.

Comparison:

- Some internal route files were named after URL shapes:
  - `internal_run_bridges.rs`
  - `internal_terminal_bridges.rs`

Gap:

- Some names described path strings rather than code responsibilities.

Implemented:

- Renamed the internal route modules to:
  - `src/api/internal/run_bridge.rs`
  - `src/api/internal/terminal_bridge.rs`
- Kept the actual HTTP paths unchanged. The file names now describe the handler module rather than the URL segment.

## Area 5: Large File Decomposition

Research:

- Splitting a module is useful when the new files expose real boundaries.
- Splitting only to reduce line count creates indirection without improving comprehension.

Comparison:

- `src/runs/service.rs` mixed:
  - service surface
  - execution loop
  - bridge protocol
  - bootstrap rendering
  - SSE mapping
- `src/terminal.rs` mixed:
  - bootstrap rendering
  - bridge state
  - websocket protocol
  - terminal service logic

Gap:

- The run and terminal domains each had several clear internal seams.

Implemented:

- Split terminal internals into:
  - `src/terminal/bootstrap.rs`
  - `src/terminal/bridge.rs`
  - `src/terminal/protocol.rs`
  - `src/terminal/service.rs`
- Split run internals into:
  - `src/runs/bootstrap.rs`
  - `src/runs/bridge.rs`
  - `src/runs/events.rs`
  - `src/runs/service.rs`
  - `src/runs/service/execution.rs`
- After the split:
  - `src/runs/service.rs` now reads as the run service surface and lightweight state.
  - `src/runs/service/execution.rs` now holds the long-running execution path.

## Deferred

- `src/artifacts/blob.rs` is still large, but it remains one cohesive backend adapter with a single responsibility: Azure Blob-backed artifact storage.
- I inspected it for the same kind of split, but the cleanest seams are internal helper groups rather than public crate structure. I left that file unchanged in this pass to avoid shuffling a stable backend without a strong boundary improvement.
