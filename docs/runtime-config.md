# Runtime Config

ADE has one application runtime environment variable:

| Name                         | Required | Used by                       | Notes                                                        |
| ---------------------------- | -------- | ----------------------------- | ------------------------------------------------------------ |
| `AZURE_SQL_CONNECTIONSTRING` | Yes      | API, `pnpm start`, migrations | The application fails fast if SQL is missing or unreachable. |

The server listen address is not environment-driven.

- Local development runs the API on `127.0.0.1:8000`.
- The container image runs the API on `0.0.0.0:8000`.
- If you need a different listen address, pass `--host` and `--port` to `node dist/server.js`.

`pnpm start` loads `.env` when present and passes only `AZURE_SQL_CONNECTIONSTRING` into the container.
