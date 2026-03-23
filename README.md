# ADE

Automatic Data Extractor.

ADE is a document operations platform for messy spreadsheets.

## Repository Layout

- `apps/web` - React web app
- `apps/api` - Fastify API and production web host
- `python/ade-engine` - extraction runtime package
- `python/ade-config-template` - configurable template package
- `infra/` - infrastructure definitions
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
| `pnpm dev:web`         | Run only the web app                                                    |
| `pnpm dev:api`         | Run only the API                                                        |
| `pnpm lint`            | Run ESLint across the repo                                              |
| `pnpm format:check`    | Check the pipeline-owned repo files with Prettier                       |
| `pnpm typecheck`       | Run the TypeScript typechecks                                           |
| `pnpm test`            | Run the local commit-stage gate: lint, unit tests, then build           |
| `pnpm test:unit`       | Run the API unit tests                                                  |
| `pnpm test:acceptance` | Run the acceptance checks for a deployed environment via `ADE_BASE_URL` |
| `pnpm package:python`  | Build the Python packages                                               |
| `pnpm build`           | Build the single local release-candidate image `ade:local`              |
| `pnpm deploy:aca`      | Deploy one image to Azure Container Apps using the repo Bicep contract  |
| `pnpm start`           | Run the built local image                                               |
| `pnpm clean`           | Remove generated local output and local images                          |
