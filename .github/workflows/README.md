# ADE Deployment Pipeline

This is the canonical deployment pipeline for `clac-ca/ade`.

The workflow lives at `.github/workflows/deployment_pipeline.yml` and has three stages:

1. Commit stage
2. Acceptance stage
3. Release stage

## Operating model

- Pull requests to `main` run only the commit stage.
- Pushes to `main` run all three stages.
- Workflow concurrency cancels superseded pull-request runs and queues `main` runs instead of canceling an in-flight deployment path.
- The commit stage runs the existing repo checks first, including Bicep lint and compilation, and then builds the release candidate image once with Buildx from source using the Dockerfile's multi-stage build. On push, that same step publishes the release candidate to GHCR and stamps standard OCI image metadata.
- Acceptance reuses that release candidate. It runs `pnpm test:acceptance --image <release-candidate-image>`, and the command manages local SQL, runs the separate migration binary, starts the same release candidate, waits for readiness, runs the checks, and tears the environment down.
- Release reuses the same release candidate, validates the Bicep deployment inputs first, passes the image to Bicep as an explicit `image=` parameter override, and then starts the separate migration job explicitly after deployment.
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
pnpm start
pnpm test:acceptance
```

Runtime config reference: [docs/runtime-config.md](../../docs/runtime-config.md)
