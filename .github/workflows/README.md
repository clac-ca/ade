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

- Pull requests to `main` run only the commit stage when deployable platform paths change.
- Pushes to `main` run all three stages when deployable platform paths change.
- Release versions use the qualifying commit timestamp converted to `America/Vancouver` for the `YYYY.M.D` calendar date and `github.run_number` for the suffix. Reruns keep the same release version.
- Workflow concurrency cancels superseded pull-request runs. For `main` pushes, GitHub keeps the active platform release running and only the newest pending qualifying push; intermediate pending platform releases are intentionally dropped.
- The commit stage runs platform-only checks first, including TypeScript typecheck, Rust/API lint, ESLint, and Bicep lint, then builds the ADE Platform release candidate image once with Buildx from source using the Dockerfile's multi-stage build and standard metadata-action tags and OCI labels.
- On push, the workflow publishes that image to `ghcr.io/<org>/ade-platform` and records the pushed digest.
- Acceptance reuses that exact immutable digest. It runs `pnpm test:acceptance --image <release-candidate-image>`, and the command manages local SQL, runs the separate migration binary, starts the same release candidate, waits for readiness, runs the checks, and tears the environment down.
- Release reuses that exact immutable digest, validates the Bicep deployment inputs first, passes the image to Bicep as an explicit `image=` parameter override, starts the separate migration job explicitly after deployment, then creates the matching `ade-platform-v...` GHCR tag from that digest, tags the commit as `ade-platform-v...`, and creates the GitHub Release.
- The app container never runs schema migrations on startup.

## Required GitHub environment variables

Create a `production` environment and set:

- `AZURE_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`

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
- Platform release suffixes can have gaps because pull request validation runs consume `github.run_number`. That is intentional; the workflows do not maintain custom release counters.

Published install shape:

```sh
pip install "ade-config @ git+https://github.com/clac-ca/ade.git@ade-engine-v2026.3.28.42#subdirectory=packages/ade-config"
```
