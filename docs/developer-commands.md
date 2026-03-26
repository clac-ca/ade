# Developer Commands

ADE supports one fixed local stack per machine.

## Local defaults

- API: `http://127.0.0.1:8000`
- Web: `http://127.0.0.1:5173`
- SQL Server: `127.0.0.1:8013`

## Commands

- `pnpm deps:up`: start local SQL Server only
- `pnpm deps:down`: stop local SQL Server
- `pnpm dev`: run SQL, execute the separate migration binary, start the Axum API, and start Vite
- `pnpm dev --port 8100`: run the web dev server on a different port
- `pnpm start`: run the built image in a local production-like environment; when no SQL connection string is configured, it manages local SQL and runs the separate migration binary first
- `pnpm start --image ghcr.io/example/ade:test --port 9000`: choose the image and published host port explicitly
- `pnpm test:acceptance`: run the acceptance checks in a self-managed local production-like environment
- `pnpm test:acceptance --url http://127.0.0.1:4100`: run the acceptance checks against an existing environment instead
- `pnpm test:acceptance --image ghcr.io/example/ade:test --port 4101`: run the self-managed acceptance harness against a specific image and host port

`pnpm dev` does not read `.env`. `pnpm start` and `pnpm test:acceptance` load `.env` when present; otherwise they manage local SQL themselves.
