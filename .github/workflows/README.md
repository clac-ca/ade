# ADE Development Pipelines

This repo currently ships two GitHub Actions pipelines:

- `.github/workflows/deployment_pipeline.yml` for the application container and Azure deployment
- `.github/workflows/python-development-pipeline.yml` for the Python package release tags

Both use the same three-stage shape:

1. Commit stage
2. Acceptance stage
3. Release stage

## Application Deployment Pipeline

This is the canonical deployment pipeline for `clac-ca/ade`.

## Operating model

- Pull requests to `main` run only the commit stage.
- Pushes to `main` run all three stages.
- Workflow concurrency cancels superseded pull-request runs, but `main` pushes can run commit and acceptance work immediately while only the production release job is serialized.
- The commit stage runs the existing repo checks first, including Bicep lint and compilation, and then builds the release candidate image once with Buildx from source using the Dockerfile's multi-stage build and standard metadata-action tags and OCI labels.
- On push, the workflow publishes that image to GHCR and records the pushed digest.
- Acceptance reuses that exact immutable digest. It runs `pnpm test:acceptance --image <release-candidate-image>`, and the command manages local SQL, runs the separate migration binary, starts the same release candidate, waits for readiness, runs the checks, and tears the environment down.
- Release reuses the same immutable digest, validates the Bicep deployment inputs first, passes the image to Bicep as an explicit `image=` parameter override, and then starts the separate migration job explicitly after deployment.
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

## Python Development Pipeline

The Python package workflow lives at `.github/workflows/python-development-pipeline.yml`.

Operating model:

- Pushes to `main` run the workflow only when the Python package paths or Python release helper paths change.
- The commit stage computes one coordinated CalVer release version for `ade-engine` and `ade-config`, rewrites a temporary release snapshot, then runs Python lint, tests, and builds.
- Acceptance rebuilds the same release snapshot and smoke-installs the built distributions in a fresh virtualenv.
- Release rechecks that the triggering SHA is still the tip of `main`, creates a release snapshot commit on detached HEAD, tags that commit, pushes the tag only, then creates the GitHub Release.
- The workflow never writes version bumps back to `main`; release metadata exists only in the tagged snapshot commit.

Published install shape:

```sh
pip install "ade-config @ git+https://github.com/clac-ca/ade.git@ade-py-v2026.3.28.42#subdirectory=packages/ade-config"
```
