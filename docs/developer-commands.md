# Developer Commands

ADE supports one fixed local stack per machine.

## Local defaults

- API: `http://127.0.0.1:8000`
- Web: `http://127.0.0.1:5173`
- SQL Server: `127.0.0.1:8013`

## Commands

- `pnpm deps:up`: start local SQL Server only
- `pnpm deps:down`: stop local SQL Server
- `pnpm dev`: run SQL, migrate the local database, start the API, and start Vite
- `pnpm dev -- --port 8100`: run the web dev server on a different port
- `pnpm start`: run the built image against a reachable SQL dependency
- `pnpm start -- --image ghcr.io/example/ade:test --port 9000`: choose the image and published host port explicitly
- `pnpm test:acceptance -- --url http://127.0.0.1:4100`: run the lightweight acceptance checks

`pnpm dev` does not read `.env`.
