# ADE Deployment Pipeline

This is the canonical deployment pipeline for `clac-ca/ade`.

It follows a strict three-stage shape:

1. **Commit Stage**
2. **Acceptance Stage**
3. **Release Stage**

The workflow lives at `.github/workflows/development_pipeline.yml`.

## Why this pipeline looks the way it does

This pipeline stays intentionally direct:

- one workflow file
- one commit-stage job
- one acceptance-stage job
- one release-stage job
- the release candidate is built once in commit stage
- pull requests stop after commit stage
- pushes to `main` continue through acceptance and release

The goal is to make the delivery logic obvious from the YAML itself.

## Operating model for ADE

### 1. `main` is the authoritative integration branch

Both pull requests to `main` and pushes to `main` run the **Commit Stage**.

Only pushes to `main` continue beyond commit stage.

That means:

- pull requests get fast technical feedback
- `main` remains the authoritative path to release
- release candidates are only published from real commits on `main`

### 2. The release candidate is the built container image

The commit stage creates the local release candidate with `pnpm build`.

On pushes to `main`, that same built candidate is tagged and pushed to public GHCR as:

```text
ghcr.io/<owner>/ade-web:sha-<commit-sha>
ghcr.io/<owner>/ade-api:sha-<commit-sha>
```

Those image refs are then reused in acceptance and release.

### 3. Acceptance tests the running release candidate

Acceptance does not rebuild the app.

It:

- pulls the published release-candidate images onto the GitHub runner
- starts them with `docker compose`
- waits for the API readiness endpoint
- runs `pnpm test:acceptance` against the running candidate

### 4. Release deploys the accepted candidate

Release deploys the same accepted image refs to production through the repo-owned Azure deployment contract in `scripts/deploy-aca.mjs`.

There is no extra rebuild, no tag resolution step, and no separate emergency-redeploy workflow in this simplified model.

## What each stage does

## Commit Stage

Purpose: reject bad changes quickly and create the release candidate.

Current ADE commit stage:

- checks out the repo
- sets up pnpm, Node, and Docker Buildx
- installs dependencies with `pnpm install --frozen-lockfile`
- runs `pnpm lint`
- runs `pnpm test:unit`
- runs `pnpm build`
- on `push` to `main`, logs into GHCR and publishes the release-candidate images

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
- starts the published release candidate on the GitHub runner with `docker compose`
- waits for `http://127.0.0.1:4100/api/readyz`
- runs `ADE_BASE_URL=http://127.0.0.1:4100 pnpm test:acceptance`
- always tears the stack down afterward

The acceptance environment is the GitHub runner itself. It is not a separate Azure staging deployment.

## Release Stage

Purpose: deploy the accepted release candidate to production.

Current ADE release stage:

- runs only on `push` to `main`
- depends on the acceptance stage
- logs into Azure with OIDC
- deploys the same accepted image refs to Azure Container Apps production with `pnpm run deploy:aca`

Because the GHCR images are public, the workflow sets `ADE_REGISTRY_SERVER=ghcr.io` and does not pass registry credentials.

## GitHub repository setup required

Create `.github/workflows/development_pipeline.yml` from the file in this directory.

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

### 3. GitHub Container Registry

The commit stage pushes release-candidate images to `ghcr.io` using `GITHUB_TOKEN`.

Set the `ade-web` and `ade-api` packages to public visibility in GitHub Packages.

### 4. Branching discipline

Use `main` as the authoritative integration branch.

If you keep pull requests:

- keep them short-lived
- keep them small
- treat them as collaboration support, not as a separate delivery path

## Local equivalents

These commands line up with the workflow:

```sh
pnpm lint
pnpm test:unit
pnpm build
pnpm start
ADE_BASE_URL=http://localhost:8000 pnpm test:acceptance
```
