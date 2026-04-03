# Release Deployment

This document describes the ADE Platform development pipeline.

The release pipeline has three stages:

1. Commit stage
2. Acceptance stage
3. Release stage

Key rules:

- The release candidate image is built once and reused.
- Acceptance runs against local SQL only.
- Migrations run through the separate `ade-migrate` binary and Azure Container App Job.
- Release order is explicit: validate the deployment inputs, deploy the release candidate, then start the migration job, then observe the job result.
- The platform release creates a Git tag and GitHub Release only after deployment and migrations succeed.
- Platform release versions use the qualifying commit timestamp converted to `America/Vancouver` for the `YYYY.M.D` calendar date and `github.run_number` for the suffix. Reruns reuse the same release version.
- If multiple qualifying pushes land on `main` while a production release is already running, GitHub keeps the active release running and only the newest pending platform release. Intermediate pending platform releases are intentionally dropped.
- The running app container never performs schema migrations on startup.
- Release passes the release candidate image to Bicep as an explicit parameter override.
- Long-lived runtime secrets are seeded manually during environment bootstrap and then reused from Azure Key Vault.
- The one-time runtime SQL user bootstrap is manual and documented in [infra/README.md](../infra/README.md).

GitHub environment variables required for release:

- `AZURE_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`
