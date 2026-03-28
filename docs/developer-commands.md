# Developer Commands

ADE supports one fixed local stack per machine.

## Local Defaults

- Web: `http://127.0.0.1:5173`
- API: `http://127.0.0.1:8000`
- SQL Server: `127.0.0.1:8013`

## Quickstart

```sh
corepack enable
pnpm install
pnpm dev
```

ADE opens at `http://127.0.0.1:5173`.

`pnpm dev` starts local SQL Server, runs the separate `ade-migrate` binary, then starts the Axum API and Vite web app on the host. Use `pnpm dev --port 8100` to change only the web port, and use `pnpm dev --no-open` to skip opening the browser.

## Daily Commands

Normal local development:

```sh
pnpm dev
pnpm dev --port 8100
pnpm dev --no-open
pnpm typecheck
pnpm lint
pnpm format
pnpm format:check
pnpm test
pnpm test:unit
pnpm test:scripts
pnpm package:python
pnpm clean
```

`pnpm lint` and `pnpm test` require Azure CLI 2.53+ with Bicep support.

Dependency-only commands:

```sh
pnpm deps:up
pnpm deps:down
```

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
- `pnpm start` and `pnpm test:acceptance` load `.env` when present; otherwise they manage local SQL automatically.
- See [runtime-config.md](runtime-config.md) for the full connection string and authentication rules.

## Standard Runtime Endpoints

- `/api/healthz`
- `/api/readyz`
- `/api/version`
