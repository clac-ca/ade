# Sandbox Environment

ADE is built around one runtime story:

1. Allocate a sandbox environment from the Azure session pool.
2. Prepare the sandbox environment.
3. Install the selected config.
4. Execute the run.

That is the canonical lifecycle. Everything else is an implementation detail.

The sandbox environment is owned by `ade-api`. It lives under `apps/ade-api/sandbox-environment/` because `ade-api` is its only consumer. It is intentionally not a `packages/` package.

## Responsibilities

ADE’s runtime is split into five responsibilities:

- sandbox environment definition: what exists in every shared environment
- sandbox environment preparation: whether setup has already been applied for the current revision
- config installation: what must be installed for the selected config
- run execution: what command runs, what gets staged, and how outputs/logs are collected
- provider adapter: how ADE talks to the session-pool management endpoint provided by Azure or the local emulator

## Environment Definition

The sandbox environment definition owns the shared execution foundation:

- connector binary
- setup script
- pinned Python runtime
- base wheelhouse
- runtime variables and secrets
- network policy

The shared environment revision is derived from those shared runtime assets, not from the selected config.

ADE packages that shared runtime as one tarball carried by the API image. During prepare, the API uploads the tarball into a vanilla Azure shell session, extracts it into the sandbox root, starts `reverse-connect`, and runs `setup.sh` through that reverse connection. The tarball already contains the pinned Python runtime at its final path, so setup never downloads Python from the internet.

`reverse-connect` still stays in `packages/reverse-connect` because it is reusable code with its own tests and binary output. The sandbox-environment build depends on that package, but it does not own its source. Its Linux binary is built inside the root platform Dockerfile and injected into the tarball during the same build graph that produces the platform image. That build step is only an artifact build, not a runtime image for the sandbox session.

## Config Installation

Config installation is separate from environment setup.

- setup prepares the shared sandbox environment
- config installation installs the selected `ade-config` wheel from its mounted path under `/mnt/data/ade/configs/<workspaceId>/<configVersionId>/`
- runs execute only after both steps succeed

The API does not upload config wheels during prepare. Local development stages config wheels separately only so the local session-pool emulator can mount them into that runtime path.

This keeps shared environment concerns separate from run-specific concerns.

## Provider Mapping

Azure still uses its own terms:

- Azure resource: `session pool`
- allocated provider runtime: `session`

ADE maps that provider runtime into its own concept:

- ADE runtime concept: `sandbox environment`

That keeps Azure terminology accurate at the boundary without making ADE’s internal model provider-led.

The API only knows one provider boundary:

- `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT`

That endpoint can point at Azure's `poolManagementEndpoint` or the local session-pool emulator. The API does not switch between separate local and Azure modes.
