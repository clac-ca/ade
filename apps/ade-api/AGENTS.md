# ADE API Notes

## Rule Of Thumb

- `apps/ade-api` is ADE's runtime boundary.
- Prefer clarity, explicit contracts, and standard Axum/Tower/Tokio patterns over clever abstraction.
- Keep operational behavior first-class: config, health, readiness, startup, shutdown, and errors should be visible in code.
- Keep the frontend a plain SPA over HTTP.
- Production migrations are a separate executable and deployment concern, not app-start side effects.

## In Practice

- Keep modules small and boundaries sharp: config, HTTP, database, and runtime lifecycle.
- Fail fast on invalid runtime configuration.
- Return predictable HTTP responses and preserve stable runtime endpoints.
- Design for testability, deployability, and observability from the start.
- Preserve the existing root workflow: `pnpm dev`, `pnpm test`, `pnpm build`, and `pnpm start`.

## Focused Commands

```sh
pnpm --filter @ade/api dev
pnpm --filter @ade/api typecheck
pnpm --filter @ade/api test
pnpm --filter @ade/api build
pnpm --filter @ade/api migrate
```
