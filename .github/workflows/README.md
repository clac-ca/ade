# ADE Development Pipelines

This repo ships two GitHub Actions development pipelines:

- `.github/workflows/platform-development-pipeline.yml` for ADE Platform image, deployment, and platform releases
- `.github/workflows/engine-development-pipeline.yml` for ADE Engine package releases

Both use the same three-stage shape:

1. Commit stage
2. Acceptance stage
3. Release stage

## ADE Platform Development Pipeline

This is the canonical platform pipeline for `clac-ca/ade`.

Operating model:

- Pushes to `main` run all three stages when deployable platform paths change.
- Release versions use the qualifying commit timestamp converted to `America/Vancouver` for the `YYYY.M.D` calendar date and `github.run_number` for the suffix. Reruns keep the same release version.
- Workflow concurrency keeps one active platform release running on `main` and only the newest pending qualifying push behind it; intermediate pending platform releases are intentionally dropped.
- The commit stage is one matrix job with two parallel legs: `pnpm test` validates source, and `pnpm build` produces the candidate.
- `pnpm build` is the only platform build path. It compiles the Bicep template and params and builds the platform image in one Docker build graph that also assembles the sandbox-environment tarball carried by that image.
- On push, that same `pnpm build` path publishes the candidate image to `ghcr.io/<org>/ade-platform`, and the build leg publishes the exact image ref/digest metadata for later stages.
- Acceptance reuses that exact immutable digest. The build leg also uploads the prebuilt local session-pool-emulator image as a workflow artifact and uploads the local config-mount fixtures that acceptance needs.
- Acceptance downloads those already-built artifacts, runs `pnpm test:acceptance --image <release-candidate-image>`, starts the same release candidate, points it at the prebuilt session-pool-emulator management endpoint, waits for readiness, runs the full upload -> run -> SSE -> output checks, and tears the environment down. It does not rebuild the platform image, publish a separate emulator package, or compile the session-pool emulator.
- Release reuses that exact immutable digest, validates the Bicep deployment inputs first, passes the image to Bicep as an explicit `image=` parameter override, starts the separate migration job explicitly after deployment, tags the commit as `ade-platform-v...`, and creates the GitHub Release. If the production sandbox secret is not configured, the release stage is skipped instead of failing.
- The app container never runs schema migrations on startup.

## Required GitHub environment variables

Create a `production` environment and set:

- `AZURE_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`
- `ADE_SANDBOX_ENVIRONMENT_SECRET`

For the one-time Azure bootstrap and first manual deployment, follow [infra/README.md](../../infra/README.md).

## Local equivalents

```sh
pnpm test
pnpm build
pnpm start
pnpm test:acceptance
```

Runtime config reference: [docs/runtime-config.md](../../docs/runtime-config.md)

## ADE Engine Development Pipeline

The ADE Engine workflow lives at `.github/workflows/engine-development-pipeline.yml`.

Operating model:

- Pushes to `main` run the workflow only when the engine/config package paths or engine release helper paths change.
- Release versions use the qualifying commit timestamp converted to `America/Vancouver` for the `YYYY.M.D` calendar date and `github.run_number` for the suffix. Reruns keep the same release version.
- The commit stage computes one coordinated CalVer release version for `ade-engine` and `ade-config`, rewrites a temporary release snapshot, then runs Python lint, tests, and builds.
- Acceptance rebuilds the same release snapshot and smoke-installs the built distributions in a fresh virtualenv.
- Release creates a release snapshot commit on detached HEAD, tags that commit as `ade-engine-v...`, smoke-installs from the published tag, then creates the GitHub Release.
- The workflow never writes version bumps back to `main`; release metadata exists only in the tagged snapshot commit.
- Recovery is by rerunning the failed workflow run. Manual dispatch is intentionally disabled to avoid accidental duplicate publications from the same SHA.

Important invariant:

- Do not casually rename the workflow files. Each release stream uses that workflow's own `github.run_number`, so renames reset the visible counter history.
- Platform release suffixes can have gaps because only qualifying pushes to `main` consume `github.run_number`. That is intentional; the workflows do not maintain custom release counters.

Published install shape:

```sh
pip install "ade-config @ git+https://github.com/clac-ca/ade.git@ade-engine-v2026.3.28.42#subdirectory=packages/ade-config"
```
