# ADE Web Notes

## Rule Of Thumb

- `apps/ade-web` is a plain SPA.
- Prefer clarity, standard React patterns, and minimal abstraction over clever architecture.
- Keep routing, server data, and UI responsibilities separate.
- The web app talks to the backend over a clean HTTP boundary.
- UX states are part of code quality: loading, error, empty, and success states should be deliberate.

## In Practice

- Use React Router for navigation and route structure.
- Use TanStack Query for server state, caching, and refetch behavior.
- Keep components focused on rendering and interaction.
- Keep API access explicit and easy to trace.
- Preserve the simple local workflow and Vite-based build path.

## Focused Commands

```sh
pnpm --filter @ade/web dev
pnpm --filter @ade/web typecheck
pnpm --filter @ade/web build
```
