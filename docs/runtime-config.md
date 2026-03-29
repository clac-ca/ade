# Runtime Config

ADE has two runtime concerns:

- SQL connectivity
- artifact storage for uploads and run outputs
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

## Artifact Storage Config

ADE stores user uploads and persisted run outputs in one durable blob container per environment.

The running API is the trusted component. It chooses blob paths, validates scoped input paths, and mints short-lived exact-blob access grants.

The runtime storage settings are:

| Name                           | Required | Used by | Notes                                                                                                   |
| ------------------------------ | -------- | ------- | ------------------------------------------------------------------------------------------------------- |
| `ADE_BLOB_ACCOUNT_URL`         | Yes      | API     | Blob service endpoint used by the API for container management and durable artifact reads and writes.   |
| `ADE_BLOB_CONTAINER`           | Yes      | API     | Private blob container that stores scoped uploads and run outputs.                                      |
| `ADE_BLOB_PUBLIC_ACCOUNT_URL`  | No       | API     | Optional browser-facing blob endpoint used when upload SAS URLs must differ from the API/runtime host.  |
| `ADE_BLOB_RUNTIME_ACCOUNT_URL` | No       | API     | Optional session-facing blob endpoint used when the session runtime needs a different reachable host.    |
| `ADE_BLOB_CORS_ALLOWED_ORIGINS` | No      | API     | Comma-separated browser origins to allow on the blob service. Used for managed local Azurite setup.     |
| `ADE_BLOB_ACCOUNT_KEY`         | Local    | API     | Shared key used only for local Azurite management and local exact-blob SAS minting.                     |
| `ADE_ARTIFACTS_ROOT`           | Fallback | API     | Filesystem fallback for internal tests or emergency local use when blob settings are intentionally omitted. |

`pnpm dev`, `pnpm start`, and managed `pnpm test:acceptance` do not use `ADE_ARTIFACTS_ROOT` in the normal path. They provision local Azurite automatically and inject the blob settings instead.

Deployed environments use Azure Blob Storage with user delegation SAS:

- browser upload grants are exact-blob `PUT` URLs
- session download grants are exact-blob read-only URLs
- session output grants are exact-blob create/write URLs
- the browser and session never receive container-wide credentials
- the API does not proxy upload or output bytes

Local development uses Azurite instead of Azure Blob Storage. Azurite does not support user delegation SAS, so ADE uses the Azurite shared key only for the local emulator path. The browser and session still receive exact-blob SAS URLs, and the API still owns path selection and container setup.

## Hosted Session Config

ADE uses one shared Azure Container Apps PythonLTS session-pool resource per environment. Local development uses a Dockerized session-pool emulator that exposes the same internal execution and file routes.

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

- local Azurite Blob Storage on `http://127.0.0.1:10000/devstoreaccount1`
- local SQL on `127.0.0.1:8013`
- local session-pool emulator on `http://127.0.0.1:8014`
- freshly packaged local `ade-engine` wheel plus an injected `ADE_CONFIG_TARGETS` mapping for:
  - `workspace-a/config-v1`
  - `workspace-b/config-v2`

Managed local blob settings are injected as:

- `ADE_BLOB_ACCOUNT_URL=http://127.0.0.1:10000/devstoreaccount1`
- `ADE_BLOB_CONTAINER=documents`
- `ADE_BLOB_ACCOUNT_KEY=<Azurite devstoreaccount1 key>`
- `ADE_BLOB_PUBLIC_ACCOUNT_URL=http://127.0.0.1:10000/devstoreaccount1`
- `ADE_BLOB_RUNTIME_ACCOUNT_URL=http://host.docker.internal:10000/devstoreaccount1`
- `ADE_BLOB_CORS_ALLOWED_ORIGINS=http://127.0.0.1:<web-port>,http://localhost:<web-port>`

`pnpm start` and `pnpm test:acceptance` load `.env` when present.

- If `ADE_BLOB_ACCOUNT_URL` is absent, they start local Azurite and inject the same managed local blob settings, except `ADE_BLOB_ACCOUNT_URL` is set to `http://host.docker.internal:10000/devstoreaccount1` so the app container can reach the emulator.
- If `AZURE_SQL_CONNECTIONSTRING` is absent, they start local SQL and synthesize the container-safe connection string.
- If `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` is absent, they start the local session-pool emulator and inject:
  - `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT=http://host.docker.internal:8014`
  - `ADE_SESSION_SECRET=ade-local-session-secret`
  - `ADE_ENGINE_WHEEL_PATH=/app/python/ade_engine.whl`
  - `ADE_CONFIG_TARGETS` mapping both sample scopes to `/app/python/ade_config.whl`
- If `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` is present, they require `ADE_SESSION_SECRET`, `ADE_ENGINE_WHEEL_PATH`, and `ADE_CONFIG_TARGETS` to already be configured in `.env` and pass them through unchanged.

Deployed environments follow the same pattern. Bicep provisions the storage account, private `documents` container, blob CORS rules, one shared session-pool endpoint, and an explicit `ADE_CONFIG_TARGETS` JSON string in the app container. The running app gets Blob Storage RBAC so it can mint user delegation SAS, but the session runtime does not receive broad storage access.

## Public Runtime API

The public runtime routes are workspace/config scoped:

- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/uploads`
- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/runs`
- `GET /api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}`
- `GET /api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/events`
- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/cancel`
- `GET /api/workspaces/{workspaceId}/configs/{configVersionId}/terminal`

`POST /uploads` is the only public upload entrypoint. It returns a server-chosen scoped file path plus direct upload instructions:

```json
{
  "uploadId": "upl_123",
  "filePath": "workspaces/workspace-a/configs/config-v1/uploads/upl_123/input.xlsx",
  "upload": {
    "method": "PUT",
    "url": "https://<account>.blob.core.windows.net/<container>/workspaces/workspace-a/configs/config-v1/uploads/upl_123/input.xlsx?<sas>",
    "headers": {
      "Content-Type": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
      "x-ms-blob-type": "BlockBlob",
      "x-ms-version": "<version>"
    },
    "expiresAt": "2026-03-29T21:15:00Z"
  }
}
```

`POST /runs` is always async and returns `202 Accepted` with a `Location` header:

```json
{
  "runId": "run_123",
  "status": "pending",
  "inputPath": "workspaces/workspace-a/configs/config-v1/uploads/upl_123/input.xlsx",
  "outputPath": null,
  "eventsUrl": "/api/workspaces/workspace-a/configs/config-v1/runs/run_123/events"
}
```

`GET /runs/{runId}/events` returns `text/event-stream` and replays persisted events by sequence. SSE event ids are the run event sequence numbers, so clients can resume with `Last-Event-ID` or the `after` query parameter.

`GET /terminal` is the only public WebSocket route. Terminal traffic is bidirectional. Run events are not multiplexed onto the terminal socket.

The product flow is:

1. Request upload instructions through `POST /uploads`.
2. Upload the file directly to Blob Storage with the returned `PUT` URL and headers.
3. Run ADE against that uploaded blob-backed file through:

```json
{
  "inputPath": "workspaces/workspace-a/configs/config-v1/uploads/upl_123/input.xlsx",
  "timeoutInSeconds": 900
}
```

4. Poll `GET /runs/{runId}` for current state or reconnect safety.
5. Stream logs and lifecycle changes from `GET /runs/{runId}/events` over SSE.
6. Read the final `outputPath` from the run resource when the run succeeds.

Public `/files` and `/executions` routes are removed. The session-pool `/files` and `/executions` endpoints remain internal implementation details for wheel staging and Python bootstrap execution.

## Network Egress

Session-pool egress is enabled by default.

That is a deliberate product choice because ADE config packages may need to call external APIs during processing.

The local emulator does not try to block outbound access either.

Any config code running in a session can reach external networks unless a future policy adds narrower controls.
