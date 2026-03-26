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
- Acceptance reuses that image. It starts local SQL, runs migrations, starts the accepted image, waits for `/api/readyz`, and runs `pnpm test:acceptance -- --url http://127.0.0.1:4100`.
- Release reuses the same image and passes it to Bicep as an explicit `image=` parameter override.

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
pnpm test:acceptance -- --url http://localhost:8000
```

Runtime config reference: [docs/runtime-config.md](../../docs/runtime-config.md)
