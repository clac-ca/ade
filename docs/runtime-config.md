# Runtime Config

ADE has two runtime concerns:

- SQL connectivity
- the hosted ADE session-pool backend

## SQL Runtime Config

| Name                         | Required | Used by                        | Notes                                                        |
| ---------------------------- | -------- | ------------------------------ | ------------------------------------------------------------ |
| `AZURE_SQL_CONNECTIONSTRING` | Yes      | API, migrations, app container | The application fails fast if SQL is missing or unreachable. |

ADE keeps SQL authentication inside that connection string. Supported values are:

- `SqlPassword` for local SQL Server development.
- `ActiveDirectoryManagedIdentity` for explicit managed identity authentication. `User ID` is the optional user-assigned managed identity client ID.
- `ActiveDirectoryDefault` for ADE's passwordless fallback chain. ADE tries `WorkloadIdentityCredential`, then `ManagedIdentityCredential`, then `DeveloperToolsCredential`. When present, `User ID` is used as the client ID for workload and managed identity resolution.

ADE does not add any extra runtime environment variables of its own for that chain, and production infra still uses explicit managed identity mode rather than `ActiveDirectoryDefault`.

## Hosted Runtime Config

ADE supports two hosted runtime backends:

- local session-pool emulation
- Azure Container Apps session pools plus Azure-managed MCP

The ADE API enables hosted runtime whenever `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` is present.
Azure auth behavior is selected only when `ADE_SESSION_POOL_RESOURCE_ID` is also present.

The steady-state hosted runtime settings are:

| Name                                   | Required                     | Used by | Notes                                                                                                              |
| -------------------------------------- | ---------------------------- | ------- | ------------------------------------------------------------------------------------------------------------------ |
| `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` | Yes                          | API     | Base URL for the session-pool data-plane routes.                                                                   |
| `ADE_SESSION_POOL_MCP_ENDPOINT`        | Yes                          | API     | MCP endpoint URL for the same hosted runtime.                                                                      |
| `ADE_RUNTIME_SESSION_SECRET`           | Yes in deployed environments | API     | Secret used to derive deterministic ADE job-session identifiers. Local development uses a fixed development value. |
| `ADE_SESSION_POOL_RESOURCE_ID`         | Azure only                   | API     | Azure ARM resource id for the session pool. When present, ADE uses Azure auth to fetch MCP credentials.            |

The hosted runtime also reads the active config wheel from one of these places:

- `ADE_ACTIVE_CONFIG_WHEEL_PATH`, when set explicitly
- `/app/python`, in the built ADE container image
- `packages/ade-config/dist`, after a local `uv build --directory packages/ade-config`

Optional active-config overrides:

- `ADE_ACTIVE_CONFIG_PACKAGE_NAME`
- `ADE_ACTIVE_CONFIG_VERSION`

ADE does not support a migration-on-startup toggle. `ade-api` never runs migrations on startup, and `ade-migrate` is the only supported migration entrypoint.

ADE keeps runtime HTTP metadata intentionally small:

- `/api/version` returns only `{ service, version }`
- released ADE Platform builds report the injected platform CalVer; local builds fall back to the app package version when no release version is injected

Build provenance such as image creation time and source revision is stored in OCI image metadata rather than the runtime API.

The server listen address is not environment-driven.

- Local development runs the API on `127.0.0.1:8000`.
- The container image runs the API on `0.0.0.0:8000`.
- If you need a different listen address, pass `--host` and `--port` to `./bin/ade-api`.

## Local Defaults

`pnpm dev` is host-based and always uses local infrastructure:

- local SQL on `127.0.0.1:8013`
- local session-pool emulator on `http://127.0.0.1:8014`

`pnpm start` and `pnpm test:acceptance` load `.env` when present.

- If `AZURE_SQL_CONNECTIONSTRING` is absent, they start local SQL and synthesize the container-safe connection string.
- If `ADE_SESSION_POOL_RESOURCE_ID` is absent, they start the local session-pool emulator and inject:
  - `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT=http://host.docker.internal:8014`
  - `ADE_SESSION_POOL_MCP_ENDPOINT=http://host.docker.internal:8014/mcp`
  - `ADE_RUNTIME_SESSION_SECRET=ade-local-session-secret`
- If `ADE_SESSION_POOL_RESOURCE_ID` is present, they require the Azure session-pool endpoints and runtime secret to already be configured in `.env` and pass them through unchanged.
