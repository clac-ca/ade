# Environment Variables

ADE runtime is configured through environment variables. `pnpm start`, CI acceptance, and Azure Container Apps use the same runtime variable names.

## Runtime Variables

| Name                                 | Required    | Default / Example                                                                                                       | Used By           | Notes                                                                                                     |
| ------------------------------------ | ----------- | ----------------------------------------------------------------------------------------------------------------------- | ----------------- | --------------------------------------------------------------------------------------------------------- |
| `HOST`                               | No          | `0.0.0.0` in containers                                                                                                 | API, `pnpm start` | Bind host. `pnpm dev` manages its own local host value.                                                   |
| `PORT`                               | No          | `8000`                                                                                                                  | API, `pnpm start` | App listen port. `pnpm start -- --port` changes the host port only.                                       |
| `AZURE_SQL_CONNECTIONSTRING`         | Yes         | `Server=127.0.0.1,8013;Database=ade;User Id=sa;Password=<LOCAL_SQL_PASSWORD>;Encrypt=false;TrustServerCertificate=true` | API, migrate job  | SQL connection string                                                                                     |
| `AZURE_STORAGEBLOB_CONNECTIONSTRING` | Conditional | `DefaultEndpointsProtocol=http;...`                                                                                     | API               | Use for local Azurite or key-based storage. Mutually exclusive with `AZURE_STORAGEBLOB_RESOURCEENDPOINT`. |
| `AZURE_STORAGEBLOB_RESOURCEENDPOINT` | Conditional | `https://<storage-account>.blob.core.windows.net/`                                                                      | API               | Use with managed identity in Azure. Mutually exclusive with `AZURE_STORAGEBLOB_CONNECTIONSTRING`.         |

## Local Usage

```sh
cp .env.example .env
```

`pnpm start` loads `.env` when present and passes the runtime values into the container.

`pnpm start -- --port <host-port>` changes only the host port published by Docker.

`pnpm dev` does not read `.env`; it bootstraps local SQL, Azurite, and runtime values itself.

`pnpm deps:up` and `pnpm deps:down` are available when you want to manage only the local dependency stack.

For `pnpm start`, shell environment variables and `--port` override values from `.env`.

## Production Usage

Azure Container Apps should set the same runtime variable names as environment variables.

Sensitive values should come from platform secrets or `secretref:` values, not from committed files.

Workflow-only variables are documented in [.github/workflows/README.md](../.github/workflows/README.md).
