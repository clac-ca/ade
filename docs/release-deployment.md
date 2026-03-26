# Release Deployment

The release pipeline has three stages:

1. Commit stage
2. Acceptance stage
3. Release stage

Key rules:

- The accepted image is built once and reused.
- Acceptance runs against local SQL only.
- Migrations run through the separate `ade-migrate` binary and Azure Container App Job.
- Release order is explicit: deploy the accepted app image, then start the migration job, then observe the job result.
- The running app container never performs schema migrations on startup.
- Release passes the accepted image to Bicep as an explicit parameter override.
- The one-time runtime SQL user bootstrap is manual and documented in [infra/README.md](../infra/README.md).

GitHub environment variables required for release:

- `AZURE_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`
