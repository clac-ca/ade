# Release Deployment

This document describes the ADE Platform development pipeline.

The release pipeline has three stages:

1. Commit stage
2. Acceptance stage
3. Release stage

Release rules:

- The commit stage is two parallel jobs: one runs `pnpm test`, and one runs `pnpm build`.
- The release candidate image is built once as `ghcr.io/<owner>/ade-platform:sha-<git-sha>` and reused.
- Acceptance resolves one target, runs the full Playwright acceptance suite against that target once, and then stops. In CI that target is the SHA-tagged release-candidate image, not Azure.
- The release stage recomputes the same SHA-tagged candidate image and release metadata inline, validates `infra/main.bicep`, deploys it with the release candidate image, and then starts the fixed migration job.
- Migrations run through the separate `ade-migrate` binary and Azure Container Apps Job.
- The running app container never performs schema migrations on startup.
- The platform release creates a Git tag and GitHub Release only after deployment and migrations succeed.
- Platform release versions use the qualifying commit timestamp converted to `America/Vancouver` for the `YYYY.M.D` calendar date and `github.run_number` for the suffix. Reruns reuse the same release version.
- If multiple qualifying pushes land on `main` while a production release is already running, GitHub keeps the active release running and only the newest pending platform release. Intermediate pending platform releases are intentionally dropped.

GitHub `production` environment variables required for release:

- `AZURE_DEPLOY_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`

Set `AZURE_DEPLOY_CLIENT_ID` from the deployment identity, not the app runtime identity:

```sh
az identity show \
  --resource-group rg-ade-prod-canadacentral-002 \
  --name id-ade-deploy-prod-canadacentral-002 \
  --query clientId \
  --output tsv
```

The first-time Azure bootstrap, Key Vault secret seeding, one-time SQL Entra admin setup, and one-time SQL user setup are manual and documented in [infra/README.md](../infra/README.md).
