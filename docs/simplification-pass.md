# Simplification Pass

This document tracks the second-pass cleanup after the blob-backed runs migration.

For each area:

- research the standard approach
- compare that with the current implementation
- list the gap
- implement the smallest clearer version

## Area 1: Artifact Storage

Research:

- Rust module organization should group related functionality and split growing files into smaller modules so the public interface stays clear:
  - <https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html>
- Axum SSE favors a direct `Sse::new(stream).keep_alive(...)` shape rather than custom response plumbing when SSE is enough:
  - <https://docs.rs/axum/latest/axum/response/sse/>
- Azure recommends user delegation SAS for Blob Storage, with narrower SAS scopes instead of broader account-level access:
  - <https://learn.microsoft.com/en-us/azure/storage/common/storage-sas-overview>
- Azurite is a local emulator path, so local-only behavior can use emulator-specific configuration without changing the production contract:
  - <https://learn.microsoft.com/en-us/azure/storage/common/storage-connect-azurite>

Comparison:

- `apps/ade-api/src/artifacts.rs` currently mixes:
  - artifact path validation
  - filesystem fallback storage
  - Azure Blob read/write
  - Azure user delegation SAS creation
  - Azurite shared-key SAS creation
  - Azurite container/CORS bootstrap
- The runtime behavior is correct, but the file is doing too many jobs at once.
- The three SAS grant methods repeat the same control flow with only endpoint, permissions, and headers changing.

Gap:

- The code is more bespoke than it needs to be because generic artifact helpers and Blob-specific implementation details live in the same file.
- The repeated SAS access builders make the behavior harder to audit.

Planned simplification:

- Split the artifact code into a small module tree with explicit responsibilities.
- Keep the public API unchanged.
- Replace the repeated SAS grant builders with one small internal helper that takes the target URL base, permissions, method, and optional content type.

Implemented:

- `apps/ade-api/src/artifacts.rs` is now the public artifact interface plus generic path helpers.
- Azure Blob logic moved to:
  - `apps/ade-api/src/artifacts/blob.rs`
  - `apps/ade-api/src/artifacts/filesystem.rs`
- The three Blob grant builders now share one internal `create_access_grant(...)` path.

Examples:

- Before: `create_browser_upload_access`, `create_download_access`, and `create_upload_access` each repeated:
  - normalize path
  - choose URL base
  - choose SAS mode
  - attach the same upload headers
- After: each public method only declares:
  - which URL base to use
  - which permissions to grant
  - whether this is `GET` or `PUT`

Result:

- Top-level artifact code is easier to scan.
- Azure-specific code is still explicit, but it is no longer mixed with generic artifact path logic.
- The SAS helper regression was caught and corrected by preserving the explicit placeholder-count tests.

## Area 2: Public vs Internal Route Modules

Research:

- Axum routers stay clearest when route trees are composed from small explicit modules and nested at the top level:
  - <https://docs.rs/axum/latest/axum/struct.Router.html>
- The public ADE transport split is intentionally:
  - runs = HTTP + SSE
  - terminal = public WebSocket
  - internal bridges = implementation details

Comparison:

- `apps/ade-api/src/routes/runs.rs` still included the internal run bridge WebSocket route.
- `apps/ade-api/src/routes/uploads.rs` still included the internal artifact upload/download handlers.
- `apps/ade-api/src/routes/terminal.rs` still included the internal terminal bridge route.

Gap:

- The public route files forced the reader to mentally separate public contract code from internal bridge code.
- That made the transport model harder to understand from the file layout alone.

Implemented:

- New internal route modules:
  - `apps/ade-api/src/routes/internal_run_bridges.rs`
  - `apps/ade-api/src/routes/internal_artifacts.rs`
  - `apps/ade-api/src/routes/internal_terminal_bridges.rs`
- `apps/ade-api/src/router.rs` now merges those modules under `/api/internal`.
- Public route files now define public handlers only.

Examples:

- Before: `routes/runs.rs` imported WebSocket upgrade types even though the public runs contract is HTTP + SSE.
- After: `routes/runs.rs` only contains async run HTTP handlers and the SSE endpoint.

Result:

- The code layout now matches the product contract.
- Internal bridge code is still explicit, but it no longer leaks into the public route modules.

## Area 3: Local Runtime Config Builders

Research:

- This area is mostly standard configuration hygiene rather than library-specific behavior:
  - use one helper for one env shape
  - avoid copy-pasted object literals
  - keep fallback chains predictable

Comparison:

- `scripts/lib/blob-env.ts` repeated the same managed-local Blob object twice with only the account URL and CORS port changing.
- `scripts/lib/session-pool-env.ts` built the same session-pool env shape in two branches.
- The configured session-pool fallback still had a surprising hard-coded `createLocalContainerAppUrl(5173)` default.
- `scripts/lib/local-runtime.ts` repeated the same three-boolean managed-dependency check in startup, log collection, success cleanup, and error cleanup.

Gap:

- The code worked, but it made the local runtime rules harder to audit because the same shape appeared in multiple branches.

Implemented:

- Added one managed-local Blob values helper in `scripts/lib/blob-env.ts`.
- Added one session-pool values helper in `scripts/lib/session-pool-env.ts`.
- Simplified the configured session-pool app URL fallback to the boring default:
  - `options.appUrl ?? localContainerAppUrl`
- Collapsed local-runtime dependency state into:
  - `managedDependencies`
  - `usesManagedLocalDependencies`

Examples:

- Before: managed local Blob env creation duplicated six env keys in two branches.
- After: the branch only chooses the few values that differ, then calls one helper.
- Before: the configured session-pool fallback used a hard-coded `5173` path helper.
- After: it uses the standard local container app URL fallback and is covered by a script test.

Result:

- Local runtime configuration reads more like data and less like branching logic.
- The fallback behavior is easier to reason about and less surprising.

## Area 4: Browser SSE Handling

Research:

- MDN shows the standard browser pattern as:
  - create one `EventSource`
  - attach named event listeners with `addEventListener(...)`
  - keep `onerror` simple
  - avoid custom transport layers when plain SSE is enough:
    - <https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events>
    - <https://developer.mozilla.org/en-US/docs/Web/API/EventSource>

Comparison:

- `apps/ade-web/src/pages/RunPocPage.tsx` already uses the right primitive: `EventSource`.
- But each named run event repeated the same local boilerplate:
  - ignore stale sources
  - record `lastEventId`
  - parse JSON
  - validate the payload
  - emit the same invalid-payload error path

Gap:

- The page was standard at the transport level, but still more bespoke than needed at the listener level.
- A reader had to scan six nearly identical blocks before getting to the event-specific behavior.

Implemented:

- Added one local `addRunEventListener(...)` helper.
- Each event listener now only states:
  - the SSE event name
  - the payload guard
  - the invalid payload message
  - the event-specific state update

Examples:

- Before: `run.created`, `run.status`, `run.log`, `run.error`, `run.result`, and `run.completed` each repeated the same parse-and-guard flow.
- After: each listener is one short block that only contains the useful behavior for that event.

Result:

- The page still uses plain `EventSource`, not a wrapper.
- The run stream logic is easier to audit because the common SSE behavior is in one helper and the per-event behavior is obvious.

## Area 5: Small Router Indirections

Research:

- This is basic Rust readability hygiene more than framework-specific design:
  - remove pass-through helpers when they add no policy
  - keep the call site direct if the helper only forwards arguments unchanged

Comparison:

- `apps/ade-api/src/router.rs` had a tiny `not_found_for_path(...)` helper that only forwarded to `not_found_for_method_path(...)`.

Gap:

- The extra helper added another name to resolve without adding behavior.

Implemented:

- Removed the pass-through helper and called `not_found_for_method_path(...)` directly from `api_not_found(...)`.

Result:

- The fallback path stays explicit without one extra level of indirection.

## Area 6: Internal Terminology in Docs

Research:

- Product docs should describe the public contract first and call out internal implementation details only when necessary.

Comparison:

- `docs/interactive-session-poc.md` still referred to `/executions` in a way that could be read as part of the current ADE product surface.

Gap:

- The code already treats `/executions` as an internal session-pool primitive, but the note did not make that boundary explicit.

Implemented:

- Updated the note so it now states:
  - public ADE surface = async runs over HTTP + SSE, terminal over WebSocket
  - session-pool `/executions` = internal runtime primitive

Result:

- The docs now match the actual transport split and reduce the chance that a reader mistakes internal session APIs for product APIs.

## Area 7: Diff Hygiene

Research:

- This is ordinary maintenance discipline rather than a library-specific rule:
  - keep commits scoped to one concern
  - avoid mixing behavioral changes with unrelated formatter churn
  - restore generated or fixture files when they did not change semantically

Comparison:

- Running formatter and generator commands during the pass touched a few unrelated files:
  - Python engine files that only changed line wrapping
  - test files that only changed wrapping
  - docs table alignment
  - fixture files that only changed end-of-file newlines

Gap:

- Those changes added review noise without improving the implementation.
- They made it harder to see the real simplifications.

Implemented:

- Reverted unrelated formatting-only edits.
- Restored fixture files byte-for-byte when the only delta was the extra trailing newline already present in `HEAD`.
- Kept only changes that either:
  - simplify a code path
  - clarify a module boundary
  - document the real public contract

Result:

- The final diff is narrower and easier to audit.
- Reviewers can focus on the architectural simplifications instead of formatter movement.
