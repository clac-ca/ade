# Developer Commands

ADE supports one fixed local stack per machine.

## Local Defaults

- Web: `http://127.0.0.1:5173`
- API: `http://127.0.0.1:8000`
- Blob Storage (Azurite): `http://127.0.0.1:10000/devstoreaccount1`
- SQL Server: `127.0.0.1:8013`
- Session Pool Emulator: `http://127.0.0.1:8014`

## Quickstart

```sh
corepack enable
pnpm install
pnpm dev
```

ADE opens at `http://127.0.0.1:5173`.

`pnpm dev` exports the shared sandbox-environment tarball from the same Docker build graph used by the platform image, stages local config wheels for the emulator when needed, starts local Azurite Blob Storage, local SQL Server, and the local session-pool emulator, builds the debug API binaries once, runs `ade-migrate`, then starts the Axum API and Vite web app on the host. Use `pnpm dev --port 8100` to change only the web port, and use `pnpm dev --no-open` to skip opening the browser.

## Daily Commands

Default developer workflow:

```sh
pnpm dev
pnpm test
pnpm test:acceptance
```

`pnpm test` is the fast commit-stage suite. It runs TypeScript type checks, schema drift checks, lint, backend tests, frontend tests, script tests, Python tests, and Bicep lint without starting the app or local infrastructure.
`pnpm build` is the single candidate builder. It compiles the Bicep template and production params and builds the local ADE Platform image `ade-platform:local` in one Docker build graph that also assembles the sandbox-environment tarball carried by the image.

`pnpm test:acceptance` is the single black-box system proof. It checks the app shell, readiness endpoints, upload -> run -> SSE -> output behavior, and scoped output isolation for both sample workspace/config pairs.

Additional local commands:

```sh
pnpm dev --port 8100
pnpm dev --no-open
pnpm --filter @ade/web gen:api
pnpm --filter @ade/web gen:api:check
pnpm format
pnpm format:check
pnpm clean
```

`pnpm test` and `pnpm build` require Azure CLI 2.53+ with Bicep support.

`pnpm --filter @ade/web gen:api` regenerates the committed frontend API schema from the backend OpenAPI contract.

`pnpm --filter @ade/web gen:api:check` verifies that the committed frontend schema is up to date without rewriting files. The generated schema keeps the real `/api/...` routes.

Dependency-only commands:

```sh
pnpm deps:up
pnpm deps:down
```

`pnpm deps:up` reuses the existing local infrastructure stack when it is already running and does not force a session-pool-emulator image rebuild.

`pnpm clean` removes local build output, Python virtualenvs and locks, ADE local containers, Compose state, and the `ade-platform:local` image.

## Production-Like Runtime Commands

```sh
pnpm build
pnpm start
pnpm start --no-open
pnpm start --image ghcr.io/example/ade-platform:test --port 9000
pnpm test:acceptance
pnpm test:acceptance --url http://127.0.0.1:4100
pnpm test:acceptance --image ghcr.io/example/ade-platform:test --port 4101
```

`pnpm build` builds the local ADE Platform image `ade-platform:local`, compiles the Bicep deployment artifacts, and accepts no extra arguments.

`pnpm start` and managed `pnpm test:acceptance` use `ade-platform:local` by default, so build first unless you pass `--image`.

`pnpm test:acceptance --url <base-url>` attaches to an existing environment instead of starting a managed one.

## Runtime Config Summary

- `pnpm dev` is host-based and does not read `.env`.
- `pnpm start` and `pnpm test:acceptance` load `.env` when present; otherwise they manage local Azurite Blob Storage, local SQL, and the local session-pool emulator automatically.
- `pnpm dev` exports the local shared sandbox-environment tarball to `.package/sandbox-environment.tar.gz` for the host API.
- `pnpm build` assembles the same sandbox-environment tarball inside the platform image instead of writing it to `.package/`.
- Local host and managed-local runtime commands also stage emulator config mounts under `.package/configs`.
- Managed local container runs keep Azurite on `host.docker.internal` for the app and rewrite direct browser Azurite links to `127.0.0.1` inside the API.
- The app always talks to `ADE_SESSION_POOL_MANAGEMENT_ENDPOINT`; local commands inject the emulator endpoint and bearer token when you have not configured Azure.
- See [runtime-config.md](runtime-config.md) for the full connection string and hosted runtime rules.

## Standard Runtime Endpoints

- `/api/healthz`
- `/api/readyz`
- `/api/version`
