# Runtime Config

This document only covers runtime environment and behavior. For the production deploy sequence, see [infra/README.md](../infra/README.md).

## Production Runtime Model

Production is intentionally simple:

- the app runs with one user-assigned managed identity
- `main.bicep` injects that identity through the standard Azure env var `AZURE_CLIENT_ID`
- the app uses that same identity for SQL, Blob Storage, and session-pool access
- `ADE_SANDBOX_ENVIRONMENT_SECRET` comes from a Key Vault reference

## Runtime Environment Variables

### SQL

| Name                         | Required | Used by         | Notes                                                                                                                                                                            |
| ---------------------------- | -------- | --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `AZURE_SQL_CONNECTIONSTRING` | Yes      | API, migrations | Production uses `Authentication=ActiveDirectoryManagedIdentity` with `User ID=<app-uami-client-id>` for the app and `User ID=<deployment-uami-client-id>` for the migration job. |

Supported SQL auth modes:

- `SqlPassword` for local SQL Server development
- `ActiveDirectoryManagedIdentity` for explicit managed identity auth
- `ActiveDirectoryDefault` for the broader Azure credential chain when needed

### Shared Azure Identity Selector

| Name              | Required | Used by | Notes                                                                                                                                                       |
| ----------------- | -------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `AZURE_CLIENT_ID` | Hosted   | API     | Standard Azure Identity env var. In production it is set to the app UAMI client ID so Blob and session-pool token acquisition use the same identity as SQL. |

### Artifact Storage

| Name                            | Required | Used by | Notes                                                                    |
| ------------------------------- | -------- | ------- | ------------------------------------------------------------------------ |
| `ADE_BLOB_ACCOUNT_URL`          | Yes      | API     | Blob service endpoint used for durable artifact reads and writes.        |
| `ADE_BLOB_CONTAINER`            | Yes      | API     | Private blob container that stores scoped uploads and run outputs.       |
| `ADE_BLOB_CORS_ALLOWED_ORIGINS` | Local    | API     | Comma-separated origins for managed local Azurite setup.                 |
| `ADE_BLOB_ACCOUNT_KEY`          | Local    | API     | Shared key used only for local Azurite management and local SAS minting. |

Hosted environments use Azure Blob Storage with user delegation SAS. Local development uses Azurite and falls back to its shared key because Azurite does not support user delegation SAS.

### Session Pool and Sandbox Secret

| Name                                   | Required | Used by | Notes                                                                                                                                                 |
| -------------------------------------- | -------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` | Yes      | API     | Base URL for the session-pool data-plane routes.                                                                                                      |
| `ADE_SESSION_POOL_BEARER_TOKEN`        | Local    | API     | Local override used for the emulator path. Hosted environments leave it unset so the app uses Azure credentials.                                      |
| `ADE_SANDBOX_ENVIRONMENT_SECRET`       | Hosted   | API     | Stable secret used to derive deterministic sandbox identifiers. Hosted production gets it through the Container App secret store backed by Key Vault. |

If `ADE_SANDBOX_ENVIRONMENT_SECRET` is missing, ADE logs one warning and generates a process-local fallback secret. That is acceptable for local ad hoc work, not for hosted production.

## Local Defaults

`pnpm dev` is host-based and always uses local infrastructure:

- local Azurite Blob Storage on `http://127.0.0.1:10000/devstoreaccount1`
- local SQL on `127.0.0.1:8013`
- local session-pool emulator on `http://127.0.0.1:8014`

`pnpm start` and `pnpm test:acceptance` load `.env` when present. If the core runtime variables are missing, they synthesize a local stack:

- local Azurite settings for Blob Storage
- local SQL connection string
- local session-pool endpoint and bearer token
- local sandbox secret

## Runtime Notes

- `ade-api` never runs migrations on startup.
- `ade-migrate` is the only supported migration entrypoint.
- The app listens on `127.0.0.1:8000` in local host mode and `0.0.0.0:8000` in the container image.
- The runtime API keeps metadata intentionally small. Build provenance stays in OCI image metadata rather than the runtime API.
