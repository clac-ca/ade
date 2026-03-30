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
