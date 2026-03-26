# ADE

ADE is a document operations platform for messy spreadsheets.

## Repository Layout

- `docs/` - runtime, developer-command, and release references
- `apps/ade-web` - React web app
- `apps/ade-api` - Fastify API and production web host
- `packages/ade-config` - installed product package and `ade` CLI
- `packages/ade-engine` - extraction runtime library used by `ade-config`
- `infra/` - Azure infrastructure definitions
- `scripts/` - root development, build, acceptance, and deployment entrypoints

## Requirements

- Node.js 24+
- pnpm 10+
- Python 3.12
- uv
- Docker

## Quickstart

```sh
corepack enable
pnpm install
pnpm dev
```

ADE opens at `http://127.0.0.1:5173`.

`pnpm dev` starts local SQL Server, runs migrations that create the local `ade` database if needed, then starts the API and web on the host.

Use `pnpm dev -- --port 8100` to change only the web port.

## Configuration

`pnpm dev` does not read `.env`.

When you want to run the built app with explicit runtime configuration, copy `.env.example` to `.env` and adjust the SQL connection string.

`pnpm start` is production-like: it expects a reachable SQL dependency and fails fast if `AZURE_SQL_CONNECTIONSTRING` is missing or the database cannot be reached.

## Root Commands

| Command                                            | Use it for                                                                                        |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `pnpm deps:up`                                     | Start only the local SQL Server dependency                                                        |
| `pnpm deps:down`                                   | Stop the local SQL Server dependency                                                              |
| `pnpm dev`                                         | Run the full watch-mode development environment                                                   |
| `pnpm lint`                                        | Run ESLint across the repo                                                                        |
| `pnpm format:check`                                | Check the pipeline-owned repo files with Prettier                                                 |
| `pnpm typecheck`                                   | Run the TypeScript typechecks                                                                     |
| `pnpm test`                                        | Run the local commit-stage gate: typecheck, lint, Node tests, Python tests, packaging, then build |
| `pnpm test:unit`                                   | Run the API and root script unit tests                                                            |
| `pnpm test:acceptance -- --url <base-url>`         | Run the acceptance checks for a running environment                                               |
| `pnpm package:python`                              | Build the Python packages                                                                         |
| `pnpm build`                                       | Build the local release-candidate image `ade:local`                                               |
| `pnpm start -- --image <image> --port <host-port>` | Run a built image against a reachable SQL dependency                                              |
| `pnpm clean`                                       | Remove generated local output, local images, and local Compose state                              |

## Docs

- [docs/runtime-config.md](docs/runtime-config.md) - application runtime configuration
- [docs/developer-commands.md](docs/developer-commands.md) - local development commands and defaults
- [docs/release-deployment.md](docs/release-deployment.md) - release pipeline overview
- [infra/README.md](infra/README.md) - Azure bootstrap and production infrastructure
- [infra/local/compose.yaml](infra/local/compose.yaml) - local SQL Server dependency stack
- [packages/ade-config/README.md](packages/ade-config/README.md) - `ade-config` package and `ade` CLI
- [packages/ade-engine/README.md](packages/ade-engine/README.md) - `ade-engine` runtime package
