# Runtime Config

ADE has two runtime concerns:

- SQL connectivity
- the hosted ADE Python session-pool backend

## SQL Runtime Config

| Name                         | Required | Used by                        | Notes                                                        |
| ---------------------------- | -------- | ------------------------------ | ------------------------------------------------------------ |
| `AZURE_SQL_CONNECTIONSTRING` | Yes      | API, migrations, app container | The application fails fast if SQL is missing or unreachable. |

ADE keeps SQL authentication inside that connection string. Supported values are:

- `SqlPassword` for local SQL Server development.
- `ActiveDirectoryManagedIdentity` for explicit managed identity authentication. `User ID` is the optional user-assigned managed identity client ID.
- `ActiveDirectoryDefault` for ADE's passwordless fallback chain. ADE tries `WorkloadIdentityCredential`, then `ManagedIdentityCredential`, then `DeveloperToolsCredential`. When present, `User ID` is used as the client ID for workload and managed identity resolution.

ADE does not add any extra runtime environment variables of its own for that chain, and production infra still uses explicit managed identity mode rather than `ActiveDirectoryDefault`.

## Hosted Session Config

ADE uses one shared Azure Container Apps PythonLTS session-pool resource per environment. Local development uses a Dockerized session-pool emulator that exposes the same execution and file routes.

The ADE API requires that session-pool config to be present at startup. Azure auth behavior is inferred from `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT`:

- `*.dynamicsessions.io` uses Azure bearer-token auth for the `https://dynamicsessions.io` audience
- any other host is treated as the local emulator and uses no auth

The steady-state hosted runtime settings are:

| Name                                   | Required | Used by | Notes                                                                                           |
| -------------------------------------- | -------- | ------- | ----------------------------------------------------------------------------------------------- |
| `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` | Yes      | API     | Base URL for the session-pool data-plane routes.                                                |
| `ADE_SESSION_SECRET`                   | Yes      | API     | Secret used to derive deterministic ADE session identifiers from `workspaceId:configVersionId`. |
| `ADE_ENGINE_WHEEL_PATH`                | Yes      | API     | Explicit wheel path for `ade-engine`.                                                           |
| `ADE_CONFIG_TARGETS`                   | Yes      | API     | JSON array mapping `{ workspaceId, configVersionId }` pairs to explicit config wheel paths.     |

ADE does not discover or build Python wheels at runtime. The dev and start scripts package wheels up front and pass their paths into the API explicitly.

`ADE_CONFIG_TARGETS` uses this shape:

```json
[
  {
    "workspaceId": "workspace-a",
    "configVersionId": "config-v1",
    "wheelPath": "/app/python/ade_config.whl"
  },
  {
    "workspaceId": "workspace-b",
    "configVersionId": "config-v2",
    "wheelPath": "/app/python/ade_config.whl"
  }
]
```

The API loads that mapping once at startup and resolves the config wheel per request from the scoped route.

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
- freshly packaged local `ade-engine` wheel plus an injected `ADE_CONFIG_TARGETS` mapping for:
  - `workspace-a/config-v1`
  - `workspace-b/config-v2`

`pnpm start` and `pnpm test:acceptance` load `.env` when present.

- If `AZURE_SQL_CONNECTIONSTRING` is absent, they start local SQL and synthesize the container-safe connection string.
- If `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` is absent, they start the local session-pool emulator and inject:
  - `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT=http://host.docker.internal:8014`
  - `ADE_SESSION_SECRET=ade-local-session-secret`
  - `ADE_ENGINE_WHEEL_PATH=/app/python/ade_engine.whl`
  - `ADE_CONFIG_TARGETS` mapping both sample scopes to `/app/python/ade_config.whl`
- If `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` is present, they require `ADE_SESSION_SECRET`, `ADE_ENGINE_WHEEL_PATH`, and `ADE_CONFIG_TARGETS` to already be configured in `.env` and pass them through unchanged.

Deployed environments follow the same pattern. Bicep injects one shared session-pool endpoint plus an explicit `ADE_CONFIG_TARGETS` JSON string into the app container. That keeps config artifact resolution request-scoped without adding a separate registry service.

The runtime proxy routes are workspace/config scoped and mirror the Azure session-pool nouns:

- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/executions`
- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/files`
- `GET /api/workspaces/{workspaceId}/configs/{configVersionId}/files`
- `GET /api/workspaces/{workspaceId}/configs/{configVersionId}/files/{filename}/content`

ADE's document-processing route sits above the generic session API:

- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/runs`

`/executions` accepts only:

- `{ "shellCommand": "pwd" }`

Each request may also include `timeoutInSeconds`. ADE injects the Azure-required `Inline` and `Synchronous` execution settings internally. For command executions, ADE translates the request server-side into Python `subprocess.run(...)` because the shared pool uses `PythonLTS`, not the Azure `Shell` container type. Raw Python execution stays internal to the `/runs` flow.

`/files` is the only upload surface. `POST /files` returns a flat file object:

```json
{
  "filename": "uploads/input.csv",
  "size": 31,
  "lastModifiedTime": "2026-03-29T12:00:00Z"
}
```

`GET /files` returns a flat array of those same objects.

`/runs` no longer accepts multipart uploads. The product flow is:

1. Upload the input file through `POST /files`.
2. Run ADE against that existing session file through:

```json
{
  "inputPath": "uploads/input.csv",
  "timeoutInSeconds": 220
}
```

`/runs` returns only:

```json
{
  "outputPath": "runs/<generated>/output/input.normalized.xlsx",
  "validationIssues": []
}
```

`/executions` returns flat ADE-shaped command output:

```json
{
  "stdout": "/mnt/data\n",
  "stderr": "",
  "exitCode": 0,
  "durationMs": 14
}
```

## Network Egress

Session-pool egress is enabled by default.

That is a deliberate product choice because ADE config packages may need to call external APIs during processing.

The local emulator does not try to block outbound access either.

Any config code running in a session can reach external networks unless a future policy adds narrower controls.
