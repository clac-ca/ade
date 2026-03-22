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
- Docker running locally for `pnpm test`, `pnpm build`, `pnpm start`, and local `pnpm test:smoke`

## Quickstart

```sh
corepack enable
pnpm install
pnpm dev
```

ADE opens at `http://localhost:8000`.

## Root Commands

| Command                | Use it for                                                                  |
| ---------------------- | --------------------------------------------------------------------------- |
| `pnpm dev`             | Run the full watch-mode development environment                             |
| `pnpm dev:web`         | Run only the web app                                                        |
| `pnpm dev:api`         | Run only the API                                                            |
| `pnpm lint`            | Run ESLint across the repo                                                  |
| `pnpm format:check`    | Check the pipeline-owned repo files with Prettier                           |
| `pnpm typecheck`       | Run the TypeScript typechecks                                               |
| `pnpm test`            | Run the authoritative local commit-stage gate                               |
| `pnpm test:unit`       | Run the API unit tests                                                      |
| `pnpm test:smoke`      | Smoke test the built local runtime, or a deployed URL via `ADE_BASE_URL`    |
| `pnpm test:acceptance` | Run the acceptance checks for a deployed environment via `ADE_BASE_URL`     |
| `pnpm package:python`  | Build the Python packages                                                   |
| `pnpm build`           | Build the local candidate artifacts and images, not the published candidate |
| `pnpm deploy:aca`      | Deploy image refs to Azure Container Apps using the repo Bicep contract     |
| `pnpm start`           | Run the built local images                                                  |
| `pnpm clean`           | Remove generated local output and local images                              |

## Common Flows

Day-to-day development:

```sh
pnpm dev
pnpm test
```

Production-shaped local run:

```sh
pnpm build
pnpm start
pnpm test:smoke
```

Commit-stage preflight before pushing:

```sh
pnpm test
```

Smoke test a deployed environment:

```sh
ADE_BASE_URL=https://example.test pnpm test:smoke
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
pnpm test
pnpm test:unit
pnpm test:smoke
ADE_BASE_URL=https://example.test pnpm test:smoke
ADE_BASE_URL=https://example.test pnpm test:acceptance
pnpm package:python
ADE_WEB_IMAGE=ghcr.io/example/ade-web@sha256:... ADE_API_IMAGE=ghcr.io/example/ade-api@sha256:... pnpm deploy:aca -- --environment acceptance --resource-group my-rg --parameters-file infra/environments/main.acceptance.bicepparam
pnpm clean
pnpm dev:web
pnpm dev:api
```

## Related Docs

- [PRINCIPLES.md](./PRINCIPLES.md) - engineering and delivery principles for this repo
- [.github/workflows/README.md](./.github/workflows/README.md) - canonical pipeline design and required GitHub/Azure setup

## Working Rules

- Work trunk-based with short-lived branches.
- `main` stays green and releasable.
- Do not add empty scaffolding.
- Do not commit generated artifacts.
- Do not add tooling unless it removes current pain.
