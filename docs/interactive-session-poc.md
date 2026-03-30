# Interactive Session POC

This note captures three concrete designs for building an interactive,
bidirectional exec channel while staying on Azure's built-in session-pool
containers.

## Current State

ADE now exposes:

- async runs over HTTP plus SSE
- an interactive terminal over a public WebSocket

Internally, the session-pool `/executions` surface is still one of the runtime
primitives used to bootstrap long-running work inside Azure sessions, but it is
not part of the public ADE API contract.

## Design 1: Reverse WebSocket PTY Bridge

This is the strongest design for terminal-like interaction on built-in
`PythonLTS`.

Flow:

1. ADE opens a relay endpoint on a host reachable from session-pool egress.
2. ADE sends a long-running inline Python payload through the internal
   session-pool `/executions` API.
3. The inline Python creates a PTY and launches a shell.
4. The inline Python opens an outbound WebSocket back to ADE.
5. ADE relays stdin, stdout, stderr, resize, and signal frames.
6. The Azure exec request stays open until the terminal closes.

Why it works well:

- true bidirectional streaming
- real TTY semantics
- no requirement for inbound ports on the built-in container
- compatible with the existing egress-enabled pool configuration

Risks:

- requires a relay endpoint reachable from Azure
- requires one long-lived exec request per active terminal
- built-in code-interpreter exec currently caps timeout at 220 seconds in the
  Azure CLI path, so long sessions need reattachment or a different keepalive
  strategy

POC:

- implemented in [scripts/poc/interactive_exec_poc.py](/Users/justinkropp/.codex/worktrees/66cd/ade/scripts/poc/interactive_exec_poc.py)
- validated locally against the Dockerized ADE session-pool emulator

## Design 2: Reverse HTTP Long-Poll PTY Bridge

This keeps the same PTY/shell model but replaces WebSocket with paired
long-poll HTTP requests.

Flow:

1. Inline Python starts a PTY shell.
2. One outbound loop posts output chunks to ADE.
3. Another outbound loop polls ADE for stdin, resize, and signal events.

Why it can be useful:

- no WebSocket dependency
- easier to proxy through strict HTTP-only environments

Tradeoffs:

- higher latency
- more chatty
- more state management on the relay side
- less natural fit for high-frequency terminal output

Verdict:

- reasonable fallback if WebSocket libraries or proxies are an issue
- not my first choice

## Design 3: File Mailbox plus Short Exec Polling

This treats the session filesystem as the transport.

Flow:

1. ADE writes command input to session files.
2. A worker process inside the session reads input files and writes output files.
3. ADE repeatedly polls files or uses short exec calls to drain output.

Why it exists:

- can work even when outbound network paths are constrained
- uses only the built-in session-pool API surface

Tradeoffs:

- weakest interactivity
- harder ordering guarantees
- brittle process lifetime assumptions
- poor fit for PTY behavior

Verdict:

- acceptable only as a lowest-common-denominator fallback
- not recommended for a user-facing shell

## Recommendation

Build around Design 1 first.

It matches the desired interactive behavior, uses the built-in `PythonLTS`
container effectively, and avoids depending on undocumented background-process
survival after the exec call returns.

## Azure Validation Notes

The live production pool is reachable at:

- `https://canadacentral.dynamicsessions.io/subscriptions/3deb80d6-e7b5-4985-bf5a-b2b978b444c1/resourceGroups/rg-ade-prod-canadacentral-002/sessionPools/sp-ade-prod-canadacentral-002`

During this POC, direct calls to that pool returned `HTTP 403` even with a
valid `https://dynamicsessions.io` bearer token. The current signed-in user does
not have a visible role assignment on the session-pool scope, which matches the
official requirement to hold the `Azure ContainerApps Session Executor` role.

Once RBAC is fixed and a public relay URL is available, the same POC harness can
exercise the live pool with `--mode azure`.
