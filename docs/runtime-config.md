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

ADE stores user uploads, persisted run outputs, and archived run logs in one durable blob container per environment.

The running API is the trusted component. It chooses blob paths, validates scoped input paths, and mints short-lived exact-blob access grants.

The runtime storage settings are:

| Name                            | Required | Used by | Notes                                                                                                                       |
| ------------------------------- | -------- | ------- | --------------------------------------------------------------------------------------------------------------------------- |
| `ADE_BLOB_ACCOUNT_URL`          | Yes      | API     | Blob service endpoint used by the API for container management and durable artifact reads and writes.                       |
| `ADE_BLOB_CONTAINER`            | Yes      | API     | Private blob container that stores scoped uploads and run outputs.                                                          |
| `ADE_BLOB_PUBLIC_ACCOUNT_URL`   | No       | API     | Optional browser-facing blob endpoint used when browser upload and download SAS URLs must differ from the API/runtime host. |
| `ADE_BLOB_RUNTIME_ACCOUNT_URL`  | No       | API     | Optional session-facing blob endpoint used when the session runtime needs a different reachable host.                       |
| `ADE_BLOB_CORS_ALLOWED_ORIGINS` | No       | API     | Comma-separated browser origins to allow on the blob service. Used for managed local Azurite setup.                         |
| `ADE_BLOB_ACCOUNT_KEY`          | Local    | API     | Shared key used only for local Azurite management and local exact-blob SAS minting.                                         |

ADE has no filesystem artifact mode. If blob settings are missing or invalid, the API fails fast.

Deployed environments use Azure Blob Storage with user delegation SAS:

- browser upload grants are exact-blob `PUT` URLs
- browser download grants are exact-blob `GET` URLs
- session download grants are exact-blob read-only URLs
- session output grants are exact-blob create/write URLs
- the browser and session never receive container-wide credentials
- the API does not proxy upload, download, or output bytes

Local development uses Azurite instead of Azure Blob Storage. Azurite does not support user delegation SAS, so ADE uses the Azurite shared key only for the local emulator path. The browser and session still receive exact-blob SAS URLs, and the API still owns path selection and container setup.

## Hosted Session Config

ADE uses one shared Azure Container Apps Shell session-pool resource per environment. Local development uses a Dockerized session-pool emulator that exposes the same internal execution and file routes.

The ADE API requires that session-pool config to be present at startup. The session-pool client always uses the pinned Shell data-plane API version `2025-10-02-preview`, and it always sends a Bearer header:

- `*.dynamicsessions.io` uses Azure bearer-token auth for the `https://dynamicsessions.io` audience
- any other host is treated as the local emulator and uses the built-in local bearer token

ADE intentionally follows the observed Shell pool behavior rather than the broader code-interpreter documentation when they diverge:

- uploads with no `path` land in the session root under `/mnt/data`
- uploads with a non-empty `path` land under `/app/<path>`
- shell commands always start in `/mnt/data`

The steady-state hosted runtime settings are:

| Name                                   | Required | Used by | Notes                                                                                           |
| -------------------------------------- | -------- | ------- | ----------------------------------------------------------------------------------------------- |
| `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` | Yes      | API     | Base URL for the session-pool data-plane routes.                                                |
| `ADE_SCOPE_SESSION_SECRET`             | Yes      | API     | Secret used to derive deterministic ADE session identifiers from `workspaceId:configVersionId`. |

ADE does not discover or build Python wheels at runtime. Instead it relies on two fixed runtime conventions relative to the app working directory:

- `.package/session-bundle/`
- `.package/session-configs/<workspaceId>/<configVersionId>/`

The session bundle contains the shared connector binary, `prepare.sh`, `run.py`, the pinned Python toolchain bundle, and the base wheelhouse used to satisfy `ade-config` dependencies such as `ade-engine`. The scope-config root contains one config wheel per workspace/config pair. Each prepared scope session uploads that wheel into the fixed host directory `/app/ade/config/`.

ADE does not support a migration-on-startup toggle. `ade-api` never runs migrations on startup, and `ade-migrate` is the only supported migration entrypoint.

ADE keeps runtime HTTP metadata intentionally small:

- `/api/version` returns only `{ service, version }`
- released ADE Platform builds report the injected platform CalVer; local builds fall back to the app package version when no release version is injected

Build provenance such as image creation time and source revision is stored in OCI image metadata rather than the runtime API.

The server listen address is not environment-driven.

- Local development runs the API on `127.0.0.1:8000`.
- The container image runs the API on `0.0.0.0:8000`.
- If you need a different listen address, pass `--host` and `--port` to `./bin/ade-api`.

## Run Scheduler Config

ADE queues run execution inside the API and starts at most a small bounded number of runs at once.

| Name                     | Required | Used by | Notes                                                              |
| ------------------------ | -------- | ------- | ------------------------------------------------------------------ |
| `ADE_RUN_MAX_CONCURRENT` | No       | API     | Maximum concurrent run executions per API instance. Defaults to 4. |

## Local Defaults

`pnpm dev` is host-based and always uses local infrastructure:

- local Azurite Blob Storage on `http://127.0.0.1:10000/devstoreaccount1`
- local SQL on `127.0.0.1:8013`
- local session-pool emulator on `http://127.0.0.1:8014`
- freshly packaged local session artifacts under:
  - `.package/session-bundle`
  - `.package/session-configs/workspace-a/config-v1`
  - `.package/session-configs/workspace-b/config-v2`

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
  - `ADE_SCOPE_SESSION_SECRET=ade-local-session-secret`
- If `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT` is present, they require `ADE_SCOPE_SESSION_SECRET` to already be configured in `.env`.

Deployed environments follow the same pattern. Bicep provisions the storage account, private `documents` container, blob CORS rules, a lifecycle policy that tiers scoped block blobs to Cool after 30 days and Archive after 180 days, and one shared session-pool endpoint. The running app gets Blob Storage RBAC so it can mint user delegation SAS, but the session runtime does not receive broad storage access.

## Public Runtime API

The public runtime routes are workspace/config scoped:

- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/uploads`
- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/uploads/batches`
- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/runs`
- `GET /api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}`
- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/downloads`
- `GET /api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/events`
- `POST /api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/cancel`
- `GET /api/workspaces/{workspaceId}/configs/{configVersionId}/terminal`

`POST /uploads` is the only public upload entrypoint. It returns a server-chosen scoped file path plus direct upload instructions:

```json
{
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

`POST /uploads/batches` returns one exact-blob upload grant per file while keeping path selection server-owned:

```json
{
  "batchId": "bat_123",
  "items": [
    {
      "fileId": "fil_123",
      "filePath": "workspaces/workspace-a/configs/config-v1/uploads/batches/bat_123/fil_123/input-a.xlsx",
      "upload": {
        "method": "PUT",
        "url": "https://<account>.blob.core.windows.net/<container>/workspaces/workspace-a/configs/config-v1/uploads/batches/bat_123/fil_123/input-a.xlsx?<sas>",
        "headers": {
          "Content-Type": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
          "x-ms-blob-type": "BlockBlob",
          "x-ms-version": "<version>"
        },
        "expiresAt": "2026-03-29T21:30:00Z"
      }
    }
  ]
}
```

`POST /runs` is always async and returns `202 Accepted` with a `Location` header:

```json
{
  "runId": "run_123",
  "status": "pending"
}
```

`POST /runs/{runId}/downloads` mints a short-lived exact-blob `GET` grant for the requested artifact:

```json
{
  "artifact": "log"
}
```

```json
{
  "filePath": "workspaces/workspace-a/configs/config-v1/runs/run_123/logs/events.ndjson",
  "download": {
    "method": "GET",
    "url": "https://<account>.blob.core.windows.net/<container>/workspaces/workspace-a/configs/config-v1/runs/run_123/logs/events.ndjson?<sas>",
    "headers": {},
    "expiresAt": "2026-03-29T21:15:00Z"
  }
}
```

`GET /runs/{runId}/events` returns `text/event-stream` for the active in-memory event feed. SSE event ids are per-run sequence numbers, so clients can resume with `Last-Event-ID` or the `after` query parameter while the run is still active on the current API instance. Durable history is archived as `events.ndjson` and downloaded through `POST /runs/{runId}/downloads`. The terminal `run.completed` event includes the final `outputPath` and `logPath`, so the browser does not need an extra detail fetch just to discover artifact locations.

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
5. Stream logs and lifecycle changes from `GET /runs/{runId}/events` over SSE while the run is active.
6. Read the final `outputPath` and `logPath` from the terminal `run.completed` event or `GET /runs/{runId}`.
7. Download the output or archived NDJSON log directly from Blob Storage through `POST /runs/{runId}/downloads`.

The bulk flow is the same model, just repeated with bounded fanout:

1. Request one batch of exact-blob upload grants through `POST /uploads/batches`.
2. Upload each file directly to Blob Storage from the browser with a bounded worker pool.
3. Call `POST /runs` once per successfully uploaded file.
4. Let the API scheduler hold excess runs in `pending` until a slot is free.
5. Poll `GET /runs/{runId}` for each active bulk row.

Public `/files` and `/executions` routes are removed. The session-pool `/files` and `/executions` endpoints remain internal implementation details for wheel staging and Python bootstrap execution.

## Network Egress

Session-pool egress is enabled by default.

That is a deliberate product choice because ADE config packages may need to call external APIs during processing.

The local emulator does not try to block outbound access either.

Any config code running in a session can reach external networks unless a future policy adds narrower controls.
