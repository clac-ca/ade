# ADE

Automatic Data Extractor.

ADE is a document operations platform for messy spreadsheets. The TypeScript apps handle the web surface and backend API. The Python packages handle the extraction runtime: a stable engine and a customizable config template.

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

## Production-Shaped Local Run

```sh
pnpm build
pnpm start
pnpm start -- --port 4000
pnpm start -- --no-open
```

## Development

```sh
pnpm dev
pnpm dev -- --port 4000
pnpm dev -- --no-open
pnpm check
pnpm lint
pnpm typecheck
pnpm test
pnpm clean
pnpm dev:web
pnpm dev:api
```

## Scripts

| Command | Description |
| --- | --- |
| `pnpm dev` | Run the watch-mode development environment |
| `pnpm lint` | Lint the TypeScript and Node code |
| `pnpm typecheck` | Run the TypeScript type checks |
| `pnpm test` | Run the automated tests |
| `pnpm build` | Build the local release-candidate images |
| `pnpm start` | Run the built release candidate |
| `pnpm smoke` | Smoke test the built runtime |
| `pnpm check` | Run the fast pre-checkin validation |
| `pnpm clean` | Remove generated local build output |
| `pnpm dev:web` | Run the web app only |
| `pnpm dev:api` | Run the API only |

## CI Registry Inputs

The commit-stage workflow publishes the release candidate to an OCI registry on `main` pushes. Configure these repository secrets:

- `OCI_REGISTRY`
- `OCI_NAMESPACE`
- `OCI_USERNAME`
- `OCI_PASSWORD`

The workflow uploads `release-candidate.json` with `commitSha`, `webDigest`, and `apiDigest`. Acceptance and production should deploy those exact digests.

## Working Rules

- Work trunk-based with short-lived branches.
- `main` stays green and releasable.
- Do not add empty scaffolding.
- Do not commit generated artifacts.
- Do not add tooling unless it removes current pain.
