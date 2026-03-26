# ADE

ADE is a document operations platform for messy spreadsheets.

## Repository Layout

- `docs/` - shared environment and operational reference docs
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

ADE opens at `http://localhost:8000`.

`pnpm dev` starts local Azurite and SQL Server dependencies, runs migrations that create the local `ade` database if needed, and then starts the API and web on the host.

Use `pnpm dev -- --port 8100` to run a second worktree on the same machine.

## Configuration

`pnpm dev` works without a `.env` file and does not read runtime config from `.env`.

When you want to run the built app with explicit runtime configuration, copy `.env.example` to `.env` and adjust the values you need.

`pnpm start` is production-like: it expects a reachable SQL dependency and fails fast if `AZURE_SQL_CONNECTIONSTRING` is missing or the database cannot be reached.

Shared runtime variable names and usage are documented in [docs/environment-variables.md](docs/environment-variables.md).

## Root Commands

| Command                | Use it for                                                                      |
| ---------------------- | ------------------------------------------------------------------------------- |
| `pnpm deps:up`         | Start only the local SQL Server and Azurite dependency stack                    |
| `pnpm deps:down`       | Stop the local SQL Server and Azurite dependency stack                          |
| `pnpm dev`             | Run the full watch-mode development environment                                 |
| `pnpm lint`            | Run ESLint across the repo                                                      |
| `pnpm format:check`    | Check the pipeline-owned repo files with Prettier                               |
| `pnpm typecheck`       | Run the TypeScript typechecks                                                   |
| `pnpm test`            | Run the local commit-stage gate: lint, unit tests, Python packaging, then build |
| `pnpm test:unit`       | Run the API unit tests                                                          |
| `pnpm test:acceptance` | Run the acceptance checks for a deployed environment via `ADE_BASE_URL`         |
| `pnpm package:python`  | Build the Python packages                                                       |
| `pnpm build`           | Build the local release-candidate image `ade:local`                             |
| `pnpm start`           | Run the built local image against an already reachable SQL dependency           |
| `pnpm clean`           | Remove generated local output, local images, and local Compose state            |

## Docs

- [docs/environment-variables.md](docs/environment-variables.md) - shared runtime environment variables
- [infra/README.md](infra/README.md) - Azure bootstrap and production infrastructure
- [infra/local/compose.yaml](infra/local/compose.yaml) - local SQL Server and Azurite dependency stack
- [packages/ade-config/README.md](packages/ade-config/README.md) - `ade-config` package and `ade` CLI
- [packages/ade-engine/README.md](packages/ade-engine/README.md) - `ade-engine` runtime package
