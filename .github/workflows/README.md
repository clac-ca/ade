# ADE Development Pipelines

This repo ships two GitHub Actions development pipelines:

- `.github/workflows/platform-development-pipeline.yml` for ADE Platform image, deployment, and platform releases
- `.github/workflows/engine-development-pipeline.yml` for ADE Engine package releases

Both use the same three-stage shape:

1. Commit stage
2. Acceptance stage
3. Release stage

## ADE Platform Development Pipeline

This is the production platform pipeline for `clac-ca/ade`.

Operating model:

- Pushes to `main` run the pipeline when deployable platform paths change.
- Release versions use the qualifying commit timestamp converted to `America/Vancouver` for the `YYYY.M.D` calendar date and `github.run_number` for the suffix. Reruns keep the same release version.
- Workflow concurrency keeps one active platform release running on `main` and only the newest pending qualifying push behind it. Intermediate pending platform releases are intentionally dropped.
- The commit stage is two parallel jobs: one runs `pnpm test`, and one runs `pnpm build`.
- `pnpm build` is the only platform build path. It builds the platform image, validates the Bicep templates used by production, and publishes the SHA-tagged release candidate image `ghcr.io/<owner>/ade-platform:sha-<git-sha>`.
- Acceptance recomputes that same SHA-tagged candidate image, installs `uv` and Playwright Chromium, runs `pnpm test:acceptance --image <release-candidate-image>`, and verifies the local stack end to end through the same Playwright acceptance suite used locally.
- Release recomputes that same SHA-tagged candidate image and release metadata inline, validates and deploys `infra/main.bicep` with `image=<release-candidate-image>`, then starts the fixed migration job.
- The running app never performs schema migrations on startup.
- The one-time Azure bootstrap, Key Vault secret seed, and first manual SQL bootstrap are documented in [infra/README.md](../../infra/README.md).

## Required GitHub environment variables

Create a `production` environment and set:

- `AZURE_DEPLOY_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`

`AZURE_DEPLOY_CLIENT_ID` must be the client ID of the deployment identity used by `azure/login`, not the app runtime identity injected into the container.

For the one-time Azure bootstrap, Key Vault secret seed, and manual SQL bootstrap, follow [infra/README.md](../../infra/README.md).

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
- Commit computes one coordinated CalVer release version for `ade-engine` and `ade-config`, rewrites a temporary release snapshot, then runs Python lint, tests, and builds.
- Acceptance rebuilds the same release snapshot and smoke-installs the distributions in a fresh virtualenv.
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
