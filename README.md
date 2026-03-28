# ADE

ADE is a document operations platform for messy spreadsheets.

## Quickstart

```sh
corepack enable
pnpm install
pnpm dev
```

ADE opens at `http://127.0.0.1:5173`.

`pnpm dev` starts local SQL Server, runs the separate `ade-migrate` binary, then starts the Axum API and Vite web app on the host.

## What Starts Locally

- Web: `http://127.0.0.1:5173`
- API: `http://127.0.0.1:8000`
- SQL Server: `127.0.0.1:8013`

Use `pnpm dev --port 8100` to change only the web port.

## Daily Repo Commands

Use these for normal local development:

```sh
pnpm dev
pnpm dev --port 8100
pnpm dev --no-open
pnpm typecheck
pnpm lint
pnpm lint:python
pnpm format
pnpm format:python
pnpm format:python:check
pnpm test
pnpm test:python
pnpm test:unit
pnpm clean
```

`pnpm lint` and `pnpm test` require Azure CLI 2.53+ with Bicep support.

If you only need the local SQL dependency:

```sh
pnpm deps:up
pnpm deps:down
```

`pnpm clean` removes local build output, Python virtualenvs and locks, ADE local containers, Compose state, and the `ade-platform:local` image.

## Production-Like Local Runtime

Use these when you want to run or test the built container locally:

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

`pnpm dev` does not read `.env`. `pnpm start` and `pnpm test:acceptance` load `.env` when present; otherwise they manage local SQL themselves. For connection string and authentication details, see [docs/runtime-config.md](docs/runtime-config.md).

## Requirements

- Node.js 24+
- pnpm 10+
- Rust 1.94.0
- Python 3.12
- uv
- Docker
- Azure CLI 2.53+ with Bicep support for `pnpm lint` and `pnpm test`

## Repo Map

- `apps/ade-web` - React web app
- `apps/ade-api` - Axum API and production web host
- `packages/ade-config` - installed business rules package
- `packages/ade-engine` - runtime library and `ade` CLI used by `ade-config`
- `infra/` - Azure infrastructure definitions
- `scripts/` - root development, build, acceptance, and deployment entrypoints

## Further Docs

- [docs/developer-commands.md](docs/developer-commands.md) - local development commands and defaults
- [docs/python-packages.md](docs/python-packages.md) - Python package structure, commands, and authoring conventions
- [docs/runtime-config.md](docs/runtime-config.md) - application runtime configuration
- [docs/release-deployment.md](docs/release-deployment.md) - release pipeline overview
- [infra/README.md](infra/README.md) - Azure bootstrap and production infrastructure
- [packages/ade-config/README.md](packages/ade-config/README.md) - `ade-config` business rules package
- [packages/ade-engine/README.md](packages/ade-engine/README.md) - `ade-engine` runtime package and `ade` CLI
