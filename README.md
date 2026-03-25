# ADE

ADE is a document operations platform for messy spreadsheets.

## Repository Layout

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

## Root Commands

| Command                | Use it for                                                              |
| ---------------------- | ----------------------------------------------------------------------- |
| `pnpm dev`             | Run the full watch-mode development environment                         |
| `pnpm lint`            | Run ESLint across the repo                                              |
| `pnpm format:check`    | Check the pipeline-owned repo files with Prettier                       |
| `pnpm typecheck`       | Run the TypeScript typechecks                                           |
| `pnpm test`            | Run the local commit-stage gate: lint, unit tests, then build           |
| `pnpm test:unit`       | Run the API unit tests                                                  |
| `pnpm test:acceptance` | Run the acceptance checks for a deployed environment via `ADE_BASE_URL` |
| `pnpm package:python`  | Build the Python packages                                               |
| `pnpm build`           | Build the local release-candidate image `ade:local`                     |
| `pnpm start`           | Run the built local image                                               |
| `pnpm clean`           | Remove generated local output, local images, and local Compose state    |

## Docs

- [infra/README.md](/Users/justinkropp/.codex/worktrees/4552/ade/infra/README.md) - Azure bootstrap and production infrastructure
- [packages/ade-config/README.md](/Users/justinkropp/.codex/worktrees/4552/ade/packages/ade-config/README.md) - `ade-config` package and `ade` CLI
- [packages/ade-engine/README.md](/Users/justinkropp/.codex/worktrees/4552/ade/packages/ade-engine/README.md) - `ade-engine` runtime package
