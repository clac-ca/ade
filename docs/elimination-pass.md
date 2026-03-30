## Elimination Pass

### Section 1: Remove unused app state and one-call session wrapper

Issue:
- `AppState` still carried `session_service` even though no route extracted it.
- `SessionService::execute_inline_python(...)` existed only to call `execute_inline_python_detailed(...).await?.value`.

Standard approach:
- In Axum, application state should only hold the fields handlers actually extract.
- Service methods should not duplicate another method when the caller can use the real method directly.

Change:
- Removed `session_service` from `AppState`.
- Removed the unused `FromRef<AppState> for Arc<SessionService>`.
- Removed `SessionService::execute_inline_python(...)`.
- Updated the terminal execution path to call `execute_inline_python_detailed(...).await?.value` directly.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- 25 lines deleted.
- No behavior change.

### Section 2: Remove bridge attachment pass-through methods

Issue:
- The internal bridge routes claimed a bridge sender and then called service methods that only did `bridge_tx.send(socket)`.
- Each route also had a dedicated `map_websocket_rejection(...)` helper used once.

Standard approach:
- If a route already owns the one-shot sender, it should complete that send itself.
- One-use error mappers should stay inline unless they hide real logic.

Change:
- Removed `RunService::attach_bridge_socket(...)`.
- Removed `TerminalService::attach_bridge_socket(...)`.
- Inlined the bridge sender call directly into the two internal WebSocket upgrade handlers.
- Inlined the WebSocket rejection mapping.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- More dead surface removed from the services.
- Internal routes now read as the direct transport boundary they actually are.

### Section 3: Remove route-local error noise and impossible bridge-url branches

Issue:
- `POST /uploads` had a custom error logging branch even though it only rethrew the same error.
- Both bridge URL builders returned `Result<String, AppError>` and handled invalid URL schemes, even though `from_env(...)` already rejects any `ADE_APP_URL` that is not `http` or `https`.

Standard approach:
- Route handlers should usually just return service errors unless they add real recovery or translation logic.
- Once a constructor validates an invariant, downstream code should use that invariant directly instead of re-checking it everywhere.

Change:
- Simplified `create_upload(...)` to return `run_service.create_upload(...).await?` directly.
- Changed the run and terminal bridge URL builders to return `String` instead of `Result<String, AppError>`.
- Removed the impossible per-request bridge URL error handling and relied on the startup-time `ADE_APP_URL` validation.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`
- `pnpm build`
- `pnpm --filter @ade/web gen:api:check`
- `pnpm test:session:local`

Result:
- The code now checks `ADE_APP_URL` once at construction time and trusts it afterward.
- The upload route is back to the normal Axum shape: parse request, call service, return JSON.

### Section 4: Remove testing-only pending-bridge inspection

Issue:
- The terminal service exposed `pending_count()` only so tests could inspect internal pending bridge state.
- The integration test also needed a second app-builder helper only to get that service handle back.

Standard approach:
- Tests should prefer observable behavior over internal counters.
- If cleanup matters, prove it at the protocol boundary instead of reading private state through a testing hook.

Change:
- Removed `PendingTerminalManager::pending_count()`.
- Removed `TerminalService::pending_count()`.
- Simplified the unit test to prove removal by checking that a cancelled bridge can no longer be claimed.
- Rewrote the integration test to extract the internal bridge URL from the bootstrap code and verify that connecting after browser disconnect fails.
- Removed the extra `app_with_session_and_url_and_terminal_service(...)` test helper and kept a single app builder.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- The production code no longer exposes a testing-only inspection method.
- The integration test is stronger because it verifies externally visible cleanup.

### Section 5: Remove one-use helper functions

Issue:
- A few helpers still only wrapped one line with no policy: terminal WebSocket rejection mapping, `ActiveRunHandle::sender()`, and `terminal_scope_key(...)`.

Standard approach:
- Inline one-use helpers when they do not hide reusable logic or improve readability.

Change:
- Inlined terminal WebSocket rejection mapping in the public route.
- Removed `ActiveRunHandle::sender()` and used the field directly.
- Removed `terminal_scope_key(...)` and inlined the `workspace:config` key formatting.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- Fewer jumps between tiny helpers.
- The remaining helpers in these files now mostly correspond to real behavior, not simple forwarding.

### Section 6: Remove more one-use helper functions

Issue:
- `session/service.rs` still had `config_for(...)` and `session_file_path(...)`, both only used from `runtime_artifacts(...)`.
- `runs/service.rs` still had `run_access_expiry(...)`, a one-line helper used in two nearby places.
- `terminal/service.rs` still had `execution_error_message(...)`, which was only used once and mainly wrapped another helper.

Standard approach:
- Keep a helper only if it hides reusable logic or a real policy.
- If a helper only forwards one expression and the call site stays readable without it, inline it.

Change:
- Inlined the config lookup and session root paths directly into `runtime_artifacts(...)`.
- Removed `run_access_expiry(...)` and computed the timestamp at the two call sites.
- Removed `execution_error_message(...)`.
- Simplified `execution_failure_message(...)` to work directly on `&PythonExecution`, then handled the success/error branch inline where the join result is consumed.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- Fewer tiny helpers in the run, session, and terminal paths.
- The remaining helpers now correspond more closely to real state transitions or reusable operations.

### Section 7: Delete one-use bridge URL builders

Issue:
- `terminal/service.rs` and `runs/service/execution.rs` each still had a dedicated `build_bridge_url(...)` helper used once.
- Those helpers were simple local string construction and no longer hid any extra policy.

Standard approach:
- If a helper is used once and the call site stays readable, prefer putting the code where the data is assembled.

Change:
- Inlined terminal bridge URL construction into `run_browser_terminal(...)`.
- Inlined run bridge URL construction into `run_attempt(...)`.
- Removed both `build_bridge_url(...)` helper methods.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- One fewer jump in each path where the bootstrap config is assembled.
- The bootstrap inputs now read straight through at the call site.

### Section 8: Delete one-field bridge entry wrappers and the last one-use terminal setup helper

Issue:
- `terminal/bridge.rs` and `runs/bridge.rs` still stored `oneshot::Sender<WebSocket>` inside one-field wrapper structs.
- `terminal/service.rs` still had `create_pending_terminal(...)`, used once.

Standard approach:
- If the map value is already the final type you need, store that type directly.
- If a helper only composes a few values once at one call site, inline it.

Change:
- Replaced the bridge-manager map values with `oneshot::Sender<WebSocket>` directly.
- Removed `PendingBridgeEntry` and `PendingRunBridgeEntry`.
- Inlined `create_pending_terminal(...)` into `run_browser_terminal(...)`.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- The bridge managers now use the simplest possible storage shape.
- Terminal startup no longer jumps through one more helper before the real bootstrap setup.

### Section 9: Remove protocol-local control-flow types

Issue:
- `terminal/protocol.rs` still carried `BrowserStartupOutcome`, `TerminalPhase`, and `TerminalRelayOutcome`.
- Only the actual wire types belong in `protocol.rs`; the startup and relay control-flow enums were service implementation details.
- `TerminalRelayOutcome` also duplicated Rust's built-in `ControlFlow`.

Standard approach:
- Keep file-local control-flow state next to the service logic that uses it.
- Prefer `std::ops::ControlFlow` over a custom `continue/close` enum when the standard type already matches.

Change:
- Moved `BrowserStartupOutcome` and `TerminalPhase` into `terminal/service.rs`.
- Replaced `TerminalRelayOutcome` with `std::ops::ControlFlow<()>`.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- `terminal/protocol.rs` is back to actual terminal protocol code.
- One custom enum is deleted and the service control flow now uses the standard library type.

### Section 10: Stop reparsing the session-pool endpoint on every request

Issue:
- `SessionPoolClient::from_env(...)` already parsed the session-pool endpoint to inspect the host.
- The client still stored the raw string and reparsed it for every request, which kept config-error branches alive in the hot path.

Standard approach:
- Parse configuration once at startup and store the typed value.
- Request code should build on validated state instead of revalidating config for every call.

Change:
- Stored the session-pool endpoint as `Url` in `SessionPoolClient`.
- Validated that it can be used as a base URL in `from_env(...)`.
- Changed `session_pool_url(...)` to build from the stored `Url` directly and removed the per-request `Result`.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- Request construction now uses typed config instead of reparsing a string.
- The request path no longer carries configuration failure branches that only belong at startup.

### Section 11: Delete the terminal startup message enum and a one-use token helper

Issue:
- `terminal/service.rs` still used `BrowserStartupOutcome` only to wrap a direct `match` on browser startup messages.
- `terminal/bridge.rs` still had a one-use `bridge_signature(...)` helper called only by `create_bridge_token(...)`.

Standard approach:
- If a helper or enum only renames a single local branch table, keep the branch table at the call site.
- One-use crypto helpers should stay inline unless they hide shared policy.

Change:
- Inlined startup browser-message handling into `wait_for_bridge_socket(...)` and `wait_for_ready_message(...)`.
- Deleted `BrowserStartupOutcome`.
- Inlined the token signature calculation into `create_bridge_token(...)`.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- One service-private enum deleted.
- One one-use bridge helper deleted.
- The terminal startup flow now reads directly from the actual browser/bridge state transitions.

### Section 12: Delete the one-use upload path splitter

Issue:
- `session/client.rs` still had `split_session_file_path(...)`, used only once in `upload_file(...)`.
- Its dedicated unit test only existed to support that helper.

Standard approach:
- If a helper only wraps a local `rsplit_once(...)` and the call site stays readable, inline it.

Change:
- Inlined the filename/directory split into `upload_file(...)`.
- Deleted `split_session_file_path(...)`.
- Deleted the helper-specific unit test.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- One function and one dedicated test deleted.
- The upload code now keeps the path split where the multipart request is assembled.

### Section 13: Delete pending-bridge wrapper structs

Issue:
- `PendingBrowserTerminal` and `PendingRunBridge` were still just small transport bundles for ids plus a one-shot receiver.

Standard approach:
- Keep ids and receivers as plain local values when that is all the caller needs.
- Let managers return the actual values the caller consumes, not one more wrapper type.

Change:
- `PendingTerminalManager::create(...)` now returns `oneshot::Receiver<WebSocket>`.
- `PendingRunBridgeManager::create(...)` now returns `(String, oneshot::Receiver<WebSocket>)`.
- Deleted `PendingBrowserTerminal` and `PendingRunBridge`.
- Updated terminal and run startup code to keep bridge ids and receivers as locals.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- Two transport-only wrapper structs deleted.
- Startup code now carries plain ids and receivers instead of extra bundle types.

### Section 14: Delete the one-use terminal spawn helper

Issue:
- `spawn_terminal_execution(...)` only wrapped one `tokio::spawn(...)` call and was used once.

Standard approach:
- When a helper only exists to name one local spawn block, inline it at the call site.

Change:
- Inlined the terminal execution `tokio::spawn(...)` block into `run_browser_terminal(...)`.
- Deleted `spawn_terminal_execution(...)`.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- One more one-use helper removed.
- Terminal startup now reads straight through from bootstrap construction to task spawn.

### Section 15: Delete the one-use session file conversion and host predicate

Issue:
- `AzureFileRecord::into_session_file(...)` only wrapped one local struct conversion in `upload_file(...)`.
- `is_dynamicsessions_host(...)` only wrapped a single host comparison used once at startup.

Standard approach:
- Keep one-off data reshaping at the point where the response is decoded if the mapping still fits on screen.
- Inline tiny startup predicates when the branch stays obvious and there is only one caller.

Change:
- Inlined `AzureFileRecord -> SessionFile` conversion into `upload_file(...)`.
- Inlined the Dynamicsessions host check into `SessionPoolClient::from_env(...)`.
- Deleted the dedicated helper and its dedicated unit test.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- Two one-use helpers deleted.
- The session upload path and startup auth inference now read directly where the values are used.

### Section 16: Delete the generic run-bridge JSON sender

Issue:
- `runs/bridge.rs` still had `send_json(...)`, but it only sent the single `Cancel` control message in two places.

Standard approach:
- If a helper only hides one direct write of one concrete payload, send that payload directly at the call site.

Change:
- Deleted `send_json(...)`.
- Inlined `RunBridgeServerMessage::Cancel` serialization and websocket send at the two cancel call sites in `execution.rs`.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- One generic bridge helper deleted.
- Run cancellation is now expressed directly where the cancel signal is emitted.

### Section 17: Delete the one-use active-session release helper

Issue:
- `ActiveTerminalManager::release(...)` was only called from `ActiveTerminalLease::drop(...)`.

Standard approach:
- If cleanup only exists to support one `Drop` impl, perform the cleanup directly in `drop(...)`.

Change:
- Deleted `ActiveTerminalManager::release(...)`.
- Inlined the hash-set removal into `ActiveTerminalLease::drop(...)`.

Validation:
- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`

Result:
- One more one-use helper removed.
- Terminal session cleanup now lives exactly where the lease ends.
