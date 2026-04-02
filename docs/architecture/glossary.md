# Glossary

This is ADE’s canonical terminology source of truth.

## Core Concepts

- `session pool`: the Azure allocator boundary. Azure allocates remote execution capacity from a session pool.
- `sandbox environment`: ADE’s container-backed execution environment. It is an API-owned runtime component, not a reusable package. ADE prepares it before it installs a config or executes a run.
- `config`: the ADE-specific package installed into a prepared sandbox environment for a run.
- `run`: the ADE job lifecycle for one input and one config.

## Runtime Lifecycle

- `allocate`: acquire a sandbox environment from the session pool.
- `prepare`: make the shared sandbox environment ready by applying the setup script and shared runtime assets.
- `install`: install the selected config into the prepared sandbox environment.
- `execute`: run ADE work inside the prepared sandbox environment.

## Sandbox Environment Contents

A sandbox environment definition owns the shared runtime ingredients:

- container image
- pre-installed packages
- environment variables
- environment secrets
- setup script
- pinned Python runtime
- network or internet policy
- revision identity

## Provider Boundary

- `session pool` stays as the provider term because Azure uses it directly.
- `sandbox environment` is ADE’s runtime term for the environment it prepares and runs in.
- `reverse-connect`, `/files`, and `/executions` are implementation details, not first-class product concepts.
