# ADE

Automatic Data Extractor.

ADE is a document operations platform for messy spreadsheets. The TypeScript apps handle the web surface and backend API. The Python packages handle the extraction runtime: a stable engine and a customizable config template.

## Requirements

- Node.js 22+
- pnpm 10+
- Python 3.12

## Quickstart

```sh
corepack enable
pnpm install
pnpm start
```

ADE opens at `http://localhost:8000`.

## Usage

```sh
pnpm start
pnpm start -- --port 4000
pnpm start -- --no-open
```

## Development

```sh
pnpm check
pnpm dev:web
pnpm dev:api
```

## Scripts

| Command | Description |
| --- | --- |
| `pnpm start` | Start ADE locally |
| `pnpm check` | Run repo validation |
| `pnpm dev:web` | Run the web app only |
| `pnpm dev:api` | Run the API only |

## Working Rules

- Work trunk-based with short-lived branches.
- `main` stays green and releasable.
- Do not add empty scaffolding.
- Do not commit generated artifacts.
- Do not add tooling unless it removes current pain.
