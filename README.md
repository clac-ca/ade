# ADE

Automatic Data Extractor.

ADE is a document operations platform for messy spreadsheets. The TypeScript apps handle the web surface and backend API. The Python packages handle the extraction runtime: a stable engine and a customizable config template.

## Repository

- `apps/web`: web application
- `apps/api`: backend API
- `packages/contracts`: shared TypeScript contracts
- `python/ade-engine`: spreadsheet extraction engine
- `python/ade-config-template`: ADE config template package

## Development

```sh
pnpm install
pnpm check
pnpm --filter @ade/web dev
pnpm --filter @ade/api dev
```

## Working Rules

- Work trunk-based with short-lived branches.
- `main` stays green and releasable.
- Do not add empty scaffolding.
- Do not commit generated artifacts.
- Do not add tooling unless it removes current pain.
