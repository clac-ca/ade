# ADE

ADE is a document operations platform for messy spreadsheets.

## Repository Layout

- `docs/` - runtime, developer-command, and release references
- `apps/ade-web` - React web app
- `apps/ade-api` - Axum API and production web host
- `packages/ade-config` - installed product package and `ade` CLI
- `packages/ade-engine` - extraction runtime library used by `ade-config`
- `infra/` - Azure infrastructure definitions
- `scripts/` - root development, build, acceptance, and deployment entrypoints

## Requirements

- Node.js 24+
- pnpm 10+
- Rust 1.94.0
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

`pnpm dev` starts local SQL Server, runs the separate migration binary, then starts the Axum API and Vite web app on the host.

Use `pnpm dev --port 8100` to change only the web port.

## Configuration

`pnpm dev` does not read `.env`.

When you want to run the built app against an explicit SQL dependency, copy `.env.example` to `.env` and adjust the SQL connection string.

`pnpm start` is production-like: it runs the built image. If `AZURE_SQL_CONNECTIONSTRING` is configured, it uses that dependency directly. If it is not configured, it manages local SQL itself and runs the separate migration binary before starting the app container.

`ade-api` never runs migrations on startup. The app container starts the app only, and `ade-migrate` is the only supported schema-mutation entrypoint.

`pnpm test:acceptance` follows the same model. By default it creates its own local acceptance environment, and `--url` switches it into attach mode for an already-running target.

The runtime API keeps application identity deliberately small at `/api/version` and exposes Prometheus metrics separately at `/metrics`. Build provenance lives in OCI image metadata rather than the runtime API.

The SQL connection string stays as a single config surface.

- `SqlPassword` is supported for local SQL Server development.
- `ActiveDirectoryManagedIdentity` is the explicit production mode. `User ID` remains the optional user-assigned managed identity client ID.
- `ActiveDirectoryDefault` is ADE's passwordless fallback chain. It tries workload identity, then managed identity, then developer tools. When present, `User ID` is used as the client ID for workload and managed identity resolution. ADE does not add any new ADE-specific environment variables.

## Root Commands

| Command                                         | Use it for                                                                                        |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `pnpm deps:up`                                  | Start only the local SQL Server dependency                                                        |
| `pnpm deps:down`                                | Stop the local SQL Server dependency                                                              |
| `pnpm dev`                                      | Run the full watch-mode development environment                                                   |
| `pnpm lint`                                     | Run ESLint across the repo                                                                        |
| `pnpm format:check`                             | Check the pipeline-owned repo files with Prettier                                                 |
| `pnpm typecheck`                                | Run the TypeScript typechecks and Rust `cargo check`                                              |
| `pnpm test`                                     | Run the local commit-stage gate: typecheck, lint, unit tests, Python tests, packaging, then build |
| `pnpm test:unit`                                | Run the Axum API tests, web tests, and root script unit tests                                     |
| `pnpm test:acceptance`                          | Run the acceptance checks in a self-managed local production-like environment                     |
| `pnpm test:acceptance --url <base-url>`         | Run the acceptance checks for a running environment                                               |
| `pnpm package:python`                           | Build the Python packages                                                                         |
| `pnpm build`                                    | Build the local release-candidate image `ade:local` from source via the Dockerfile                |
| `pnpm start --image <image> --port <host-port>` | Run a built image in a local production-like environment                                          |
| `pnpm clean`                                    | Remove generated local output, local images, and local Compose state                              |

## Docs

- [docs/runtime-config.md](docs/runtime-config.md) - application runtime configuration
- [docs/developer-commands.md](docs/developer-commands.md) - local development commands and defaults
- [docs/release-deployment.md](docs/release-deployment.md) - release pipeline overview
- [infra/README.md](infra/README.md) - Azure bootstrap and production infrastructure
- [infra/local/compose.yaml](infra/local/compose.yaml) - local SQL Server dependency stack
- [packages/ade-config/README.md](packages/ade-config/README.md) - `ade-config` package and `ade` CLI
- [packages/ade-engine/README.md](packages/ade-engine/README.md) - `ade-engine` runtime package
