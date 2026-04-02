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

`pnpm dev` rebuilds the shared sandbox-environment tarball only when it is stale, stages local config wheels for the emulator when needed, starts local Azurite Blob Storage, local SQL Server, and the local session-pool emulator, runs the separate `ade-migrate` binary, then starts the Axum API and Vite web app on the host. Use `pnpm dev --port 8100` to change only the web port, and use `pnpm dev --no-open` to skip opening the browser.

## Daily Commands

Normal local development:

```sh
pnpm dev
pnpm check
pnpm dev --port 8100
pnpm dev --no-open
pnpm --filter @ade/web gen:api
pnpm --filter @ade/web gen:api:check
pnpm typecheck
pnpm lint
pnpm format
pnpm format:check
pnpm test
pnpm test:unit
pnpm test:session:local
pnpm test:scripts
pnpm clean
```

`pnpm lint` and `pnpm test` require Azure CLI 2.53+ with Bicep support.

`pnpm --filter @ade/web gen:api` regenerates the committed frontend API schema from the backend OpenAPI contract.

`pnpm --filter @ade/web gen:api:check` verifies that the committed frontend schema is up to date without rewriting files. The generated schema keeps the real `/api/...` routes.

`pnpm check` is the fast repo-level verification path. It runs TypeScript checks, backend `cargo check`, and frontend API schema drift checks without building the production image.

Dependency-only commands:

```sh
pnpm deps:up
pnpm deps:down
```

`pnpm deps:up` starts the local infrastructure stack without forcing a session-pool image rebuild when the existing image is already usable.

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

`pnpm build` builds the local ADE Platform image `ade-platform:local` and accepts no extra arguments.

`pnpm start` and managed `pnpm test:acceptance` use `ade-platform:local` by default, so build first unless you pass `--image`.

`pnpm test:acceptance --url <base-url>` attaches to an existing environment instead of starting a managed one.

## Runtime Config Summary

- `pnpm dev` is host-based and does not read `.env`.
- `pnpm start` and `pnpm test:acceptance` load `.env` when present; otherwise they manage local Azurite Blob Storage, local SQL, and the local session-pool emulator automatically.
- `pnpm test:session:local` is the black-box smoke command for the local session-pool path.
- `pnpm build:sandbox-environment` builds only the shared sandbox-environment tarball at `.package/sandbox-environment.tar.gz`.
- Local host and managed-local runtime commands also stage emulator config mounts under `.package/configs`.
- ADE uses Azure session pools only when the Azure session-pool settings are explicitly configured; otherwise it falls back to the local emulator.
- See [runtime-config.md](runtime-config.md) for the full connection string and hosted runtime rules.

## Standard Runtime Endpoints

- `/api/healthz`
- `/api/readyz`
- `/api/version`
