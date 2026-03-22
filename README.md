# ADE

Automatic Data Extractor.

ADE is a document operations platform for messy spreadsheets. The TypeScript apps handle the web surface and backend API. The Python packages handle the extraction runtime: a stable engine and a customizable config template.

## Repository Layout

- `apps/web` - React web app
- `apps/api` - Fastify API
- `python/ade-engine` - extraction runtime package
- `python/ade-config-template` - configurable template package
- `infra/` - infrastructure definitions
- `scripts/` - root development, build, and smoke-test entrypoints

## Requirements

- Node.js 22+
- pnpm 10+
- Python 3.12
- Docker running locally for `pnpm build`, `pnpm start`, and `pnpm smoke`

## Quickstart

```sh
corepack enable
pnpm install
pnpm dev
```

ADE opens at `http://localhost:8000`.

## Root Commands

| Command               | Use it for                                                              |
| --------------------- | ----------------------------------------------------------------------- |
| `pnpm dev`            | Run the full watch-mode development environment                         |
| `pnpm dev:web`        | Run only the web app                                                    |
| `pnpm dev:api`        | Run only the API                                                        |
| `pnpm lint`           | Run ESLint across the repo                                              |
| `pnpm format:check`   | Check the pipeline-owned repo files with Prettier                       |
| `pnpm typecheck`      | Run the TypeScript typechecks                                           |
| `pnpm test:unit`      | Run the API unit tests                                                  |
| `pnpm package:python` | Build the Python packages                                               |
| `pnpm ci:preflight`   | Run the fast local commit-stage contract without building images        |
| `pnpm ci:commit`      | Run the full commit-stage contract, including the local candidate build |
| `pnpm build`          | Build the local candidate images                                        |
| `pnpm start`          | Run the built local images                                              |
| `pnpm smoke`          | Smoke test the built local runtime                                      |
| `pnpm clean`          | Remove generated local output and local images                          |

## Common Flows

Day-to-day development:

```sh
pnpm dev
pnpm ci:preflight
```

Production-shaped local run:

```sh
pnpm build
pnpm start
pnpm smoke
```

Commit-stage preflight before pushing:

```sh
pnpm ci:preflight
```

Run on a different port or skip opening the browser:

```sh
pnpm start -- --port 4000
pnpm start -- --no-open
```

## Development Options

```sh
pnpm dev
pnpm dev -- --port 4000
pnpm dev -- --no-open
pnpm lint
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm package:python
pnpm ci:preflight
pnpm ci:commit
pnpm clean
pnpm dev:web
pnpm dev:api
```

## Related Docs

- [PRINCIPLES.md](/Users/justinkropp/.codex/worktrees/dfe6/ade/PRINCIPLES.md) - engineering and delivery principles for this repo

## Working Rules

- Work trunk-based with short-lived branches.
- `main` stays green and releasable.
- Do not add empty scaffolding.
- Do not commit generated artifacts.
- Do not add tooling unless it removes current pain.
