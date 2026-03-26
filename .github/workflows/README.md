# ADE Deployment Pipeline

This is the canonical deployment pipeline for `clac-ca/ade`.

The workflow lives at `.github/workflows/deployment_pipeline.yml` and has three stages:

1. Commit stage
2. Acceptance stage
3. Release stage

## Operating model

- Pull requests to `main` run only the commit stage.
- Pushes to `main` run all three stages.
- The commit stage builds `ade:local`, then tags and publishes the accepted image on push.
- Acceptance reuses that image. It runs `pnpm test:acceptance --image <accepted-image>`, and the command manages local SQL, runs the separate migration binary, starts the accepted image, waits for readiness, runs the checks, and tears the environment down.
- Release reuses the same image, passes it to Bicep as an explicit `image=` parameter override, and then starts the separate migration job explicitly after deployment.
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
