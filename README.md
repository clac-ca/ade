# ADE

Automatic Data Extractor.

ADE is a document operations platform for messy spreadsheets.

## Repository Layout

- `apps/ade-web` - React web app
- `apps/ade-api` - Fastify API and production web host
- `packages/ade-engine` - extraction runtime CLI/package
- `packages/ade-config-template` - configurable template package
- `infra/` - infrastructure definitions
- `infra/README.md` - Azure infrastructure setup and first deployment guide
- `scripts/` - root development, build, acceptance, and deployment entrypoints

## Requirements

- Node.js 22+
- pnpm 10+
- Python 3.12
- Docker running locally for `pnpm test`, `pnpm build`, and `pnpm start`

## Quickstart

```sh
corepack enable
pnpm install
pnpm dev
```

ADE opens at `http://localhost:8000`.

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
| `pnpm build`           | Build the single local release-candidate image `ade:local`              |
| `pnpm start`           | Run the built local image                                               |
| `pnpm clean`           | Remove generated local output and local images                          |

## Azure Production Bootstrap

The Azure production setup is intentionally direct and documented in [infra/README.md](/Users/justinkropp/.codex/worktrees/4552/ade/infra/README.md).

Keep these values out of tracked files:

- tenant ID
- subscription ID

Store them only in the GitHub `production` environment variables used by the deployment pipeline.
