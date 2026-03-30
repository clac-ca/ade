# Correctness Pass

This document tracks the fifth-pass work focused on real-world correctness:

- failures
- retries
- concurrency
- partial state

For each issue:

- identify the risk
- confirm the standard approach
- implement the smallest predictable fix

## Issue 1: Abandoned Run Attempts Left Session Tasks Running

Research:

- Tokio `JoinHandle` tasks keep running if the handle is dropped:
  - <https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html>
- Tokio’s shutdown guidance is to tell tasks to shut down and wait for them to finish, rather than leaving them detached:
  - <https://tokio.rs/tokio/topics/shutdown>

Risk:

- `run_attempt(...)` spawns the session execution task before the run bridge is fully established.
- If the attempt is abandoned before that task is joined, dropping the `JoinHandle` detaches the task.
- In practice this means a cancelled or failed attempt could keep running in the background and still:
  - download the input artifact
  - connect its bridge late
  - continue work after the API has already moved on

Standard approach:

- Abort background work when the owning flow abandons it.
- Do not let orphaned tasks continue after cancellation or early failure.

Implemented:

- Added explicit task abort-and-await handling before returning from abandoned attempt paths in:
  - `apps/ade-api/src/runs/service/execution.rs`
- Covered both:
  - failure while waiting for the bridge
  - early bridge protocol failures after the socket attaches

Result:

- Cancelled and failed attempts no longer leave detached session work running in the background.

## Issue 2: Cancellation Could Lose to Late Success/Failure Finalization

Research:

- Azure retry guidance says transient faults should retry within a bounded policy, but non-transient or cancelled operations should stop and report the terminal outcome:
  - <https://learn.microsoft.com/en-us/azure/architecture/patterns/retry>

Risk:

- The run loop checked cancellation before an attempt started, but not again right before final success/failure commit.
- A cancellation request arriving after an attempt produced a result but before final state persistence could still be overwritten by `succeeded` or `failed`.

Standard approach:

- Treat cancellation as a terminal signal and re-check it before committing final state.

Implemented:

- Re-checked `active.is_cancelled()` in `execute_run(...)` before:
  - `finish_success(...)`
  - `finish_failure(...)`

Result:

- If cancellation wins before final commit, the run finishes as `cancelled`.

## Issue 3: Failed or Cancelled Runs Could Keep Stale Output State

Risk:

- During an attempt, `run.result` updates `outputPath` and `validationIssues` immediately.
- If that same attempt is later cancelled or fails, the stored run could still expose:
  - an `outputPath`
  - validation issues
- That creates partial-state confusion: the final status says one thing, but the detail payload still looks partially successful.

Standard approach:

- Terminal failed/cancelled state should clear transient success fields unless the system explicitly supports partial success as a first-class state.
- ADE does not expose a partial-success public state, so the stored run should stay internally consistent.

Implemented:

- Cleared `output_path` and `validation_issues` in:
  - `finish_cancelled(...)`
  - `finish_failure(...)`

Result:

- Final `failed` and `cancelled` runs no longer present stale success artifacts in `GET /runs/{runId}`.

## Issue 4: Retry Behavior Needed Explicit Coverage

Risk:

- The code already had bounded retries for transient bridge-startup failures, but there was no direct test proving that a first transient failure:
  - retries exactly once
  - succeeds cleanly
  - does not duplicate final artifact writes

Standard approach:

- Keep retries bounded and only use them for early, transient failures.
- Verify the retry path explicitly in tests.

Implemented:

- Added an integration test that simulates a first run bridge connection that disconnects before sending `ready`, then verifies the second attempt succeeds cleanly.

Result:

- The retry path is now covered and remains bounded and predictable.

## Added Tests

Added integration coverage in `apps/ade-api/tests/session_contract.rs` for:

- `cancelling_before_bridge_ready_stops_the_session_attempt`
- `cancelling_after_a_partial_result_clears_stale_output_state`
- `transient_run_bridge_startup_failures_retry_once_and_then_succeed`

Validation:

- `cargo test --locked --manifest-path apps/ade-api/Cargo.toml`
- `pnpm test:session:local`
