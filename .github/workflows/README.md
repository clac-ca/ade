# ADE Deployment Pipeline

This is the canonical deployment pipeline for `clac-ca/ade`.

It follows a strict three-stage shape:

1. **Commit Stage**
2. **Acceptance Stage**
3. **Release Stage**

The workflow lives at `.github/workflows/deployment_pipeline.yml`.

## Operating model for ADE

`main` is the authoritative integration branch.

Both pull requests to `main` and pushes to `main` run the **Commit Stage**.

Only pushes to `main` continue beyond commit stage.

That means:

- pull requests get fast technical feedback
- `main` remains the authoritative path to release
- release candidates are only published from real commits on `main`

The commit stage creates the local release candidate with `pnpm build`.

On pushes to `main`, that same built candidate is tagged and pushed to public GHCR as:

```text
ghcr.io/<owner>/ade:sha-<commit-sha>
```

Those image refs are then reused in acceptance and release.

Acceptance does not rebuild the app.

It:

- pulls the published release-candidate image onto the GitHub runner
- starts Azurite and SQL Server with the root `compose.yaml`
- runs the accepted image once to create the local `ade` database
- runs the accepted image again to apply migrations
- starts the accepted image as the running app with `docker run`
- waits for the API readiness endpoint
- runs `pnpm test:acceptance` against the running candidate

Compose owns only the local dependencies in this stage. The accepted image stays separate so the workflow makes it explicit which container is the system under test.

## Commit Stage

Purpose: reject bad changes quickly and create the release candidate.

Current ADE commit stage:

- checks out the repo
- sets up pnpm, Node, and Docker Buildx
- installs dependencies with `pnpm install --frozen-lockfile`
- runs `pnpm lint`
- runs `pnpm test:unit`
- runs `pnpm build`
- on `push` to `main`, logs into GHCR and publishes the release-candidate image

This stage runs for:

- `pull_request` to `main`
- `push` to `main`

On pull requests, the publish step is skipped.

## Acceptance Stage

Purpose: prove the published release candidate works as a running system.

Current ADE acceptance stage:

- runs only on `push` to `main`
- depends on the commit stage outputs
- checks out the repo
- installs dependencies
- starts Azurite and SQL Server with `docker compose`
- runs `node dist/ensure-dev-db.js` inside the accepted image on the Compose network
- runs `node dist/migrate.js` inside the accepted image on the Compose network
- starts the published release candidate on the GitHub runner with `docker run`
- waits for `http://127.0.0.1:4100/api/readyz`
- runs `ADE_BASE_URL=http://127.0.0.1:4100 pnpm test:acceptance`
- always stops the app container and tears down the Compose project afterward

The acceptance environment is the GitHub runner itself. It is not a separate Azure staging deployment.

The app still receives the same standard env names it uses everywhere else:

- `AZURE_SQL_CONNECTIONSTRING`
- `AZURE_STORAGEBLOB_CONNECTIONSTRING`

There are no acceptance-specific application env names in the runtime contract.

## Release Stage

Purpose: deploy the accepted release candidate to production.

Current ADE release stage:

- runs only on `push` to `main`
- depends on the acceptance stage
- logs into Azure with OIDC
- sets `ADE_IMAGE` to the accepted release-candidate image ref
- deploys Azure infrastructure, the public Container App, and the manual migration job with `az deployment group create --parameters infra/environments/main.prod.bicepparam`
- starts the manual migration job with `az containerapp job start`
- polls the job execution and fails the release if migrations fail

Because the GHCR image is public, the release deployment does not configure registry credentials.

The split is intentional:

- Bicep owns resource creation
- the workflow owns the imperative "run migrations now" step

That keeps the infrastructure definition declarative and keeps migration execution visible in `release_stage`.

## GitHub repository setup required

Then configure the repo like this:

### 1. Production environment

Create a GitHub environment named:

```text
production
```

Set these variables on that environment:

- `AZURE_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`

### 2. Azure authentication

The workflow uses `azure/login` with OIDC.

Configure Azure federated credentials for the `production` GitHub environment instead of storing long-lived cloud secrets in the repository.

For the one-time Azure setup and first manual deployment, follow [infra/README.md](/Users/justinkropp/.codex/worktrees/4552/ade/infra/README.md).

### 3. GitHub Container Registry

The commit stage pushes the release-candidate image to `ghcr.io` using `GITHUB_TOKEN`.

Set the `ade` package to public visibility in GitHub Packages.

## Local equivalents

These commands line up with the workflow:

```sh
pnpm lint
pnpm test:unit
pnpm build
pnpm start
ADE_BASE_URL=http://localhost:8000 pnpm test:acceptance
```
