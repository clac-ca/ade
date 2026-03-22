# ADE Deployment Pipeline

This is the **pure Farley, intentionally simple** pipeline for `clac-ca/ade`.

It is deliberately built around **three stages only**:

1. **Commit stage**
2. **Acceptance stage**
3. **Release stage**

Nothing else is treated as authoritative.

## Why this pipeline looks the way it does

This pipeline follows Dave Farley and Jez Humble’s deployment-pipeline model very closely:

- every check-in to the shared trunk creates a pipeline instance and a release candidate
- the **commit stage** is the fast technical gate
- the **acceptance stage** proves the candidate works in a production-shaped deployment
- the **release stage** deploys the same accepted candidate to production without rebuilding it
- the same candidate is promoted forward; it is **not rebuilt for release**
- if the mainline build goes red, the team **stops the line** and fixes or reverts immediately

This repository is still small, so the pipeline is kept intentionally compact:

- **one canonical workflow file**
- **one commit stage implemented as 3 jobs**
- **one acceptance-stage job**
- **one release-stage job**
- **no PR CI bureaucracy in the canonical pipeline**
- **no duplicated test suites across multiple stages**
- **no non-essential stages yet**

That is not an omission. It is the point.

## Pure Farley operating model for ADE

### 1. Main is the authoritative integration branch

The authoritative deployment pipeline starts when a change lands on `main`.

That means:

- developers integrate in **small batches**
- branches, if used at all, stay **very short-lived**
- the authoritative automated feedback is on `main`
- if you keep pull requests, keep them tiny and fast; they are a collaboration tool, not the authoritative pipeline

### 2. The team owns the red build

If `commit-stage` or `acceptance-stage` fails on `main`:

- stop feature work
- fix forward immediately **or revert immediately**
- only resume normal work once `main` is green again

### 3. Release means promoting an accepted version

A version is releasable only after:

- `commit-stage` passed on `main`
- `acceptance-stage` passed on `main`
- the workflow created an immutable Git tag of the form:

```text
candidate-<40-char-sha>
```

Normal release on `main` is then:

- automatically deploy the accepted candidate to production
- smoke test production immediately
- create an immutable release tag if production smoke passes

Emergency redeploy is still explicit:

- choose the accepted candidate SHA
- run the workflow manually with `candidate_tag`
- let the workflow redeploy that exact accepted candidate to production without rebuilding it

That is the Farley model: **build once, test once per stage, then promote**.

## What each stage does

## Commit stage

Purpose: reject bad changes quickly and produce an immutable release candidate.

Current ADE commit stage does the following:

- checks out the repo
- uses the repo-pinned **Node 22.12.0** and **Python 3.12** toolchain
- installs JavaScript dependencies with `pnpm install --frozen-lockfile` in the verification and build/smoke jobs
- runs fast verification (`lint`, `format:check`, `typecheck`, `test:unit`, `package:python`) in parallel with candidate build and smoke
- smoke tests the exact built images via the repo-owned `pnpm run test:smoke` contract
- pushes immutable candidate image tags to GHCR:

```text
ghcr.io/<owner>/<repo>-web:sha-<commit-sha>
ghcr.io/<owner>/<repo>-api:sha-<commit-sha>
```

- publishes those image tags only after both verification and smoke pass
- only then allows the acceptance stage to begin
- uploads a `candidate-manifest.json` artifact for traceability
- carries the accepted digests forward into the annotated `candidate-<sha>` tag so release can resolve the candidate from Git alone

### What belongs here today

Exactly the checks that are already meaningful and fast in this repo:

- TypeScript compile/build checks
- API unit tests
- Python package builds
- image creation
- tiny runtime smoke

### What does **not** belong here yet

Do **not** add these to the commit stage yet:

- browser end-to-end suites
- performance tests
- long-running integration environments
- heavyweight security scans that add material latency
- tool-driven “quality theatre” that is not yet part of your definition of deployable

If a check is not fast, deterministic, and clearly valuable right now, it stays out.

## Acceptance stage

Purpose: deploy the exact release candidate and prove it works in a production-shaped acceptance environment.

Current ADE acceptance stage does the following:

- deploys the exact immutable candidate images created in commit stage to the `acceptance` Azure Container Apps environment
- runs the deployed-environment smoke test via the same repo-owned `pnpm run test:smoke` contract, pointed at `ADE_BASE_URL`
- runs the acceptance contract via the repo-owned `pnpm run test:acceptance` contract against the same deployed URL
- if that passes, creates the immutable accepted-candidate tag:

```text
candidate-<commit-sha>
```

### Why the acceptance stage is still small right now

Because ADE is still early.

At the moment, the repository has:

- a small Fastify API
- a small Vite/React web app
- a concrete Azure Container Apps deployment target in `infra/`
- a deployed smoke contract for `/`, `/api/healthz`, `/api/readyz`, and `/api/version`
- a minimal acceptance contract that currently stays close to those core behaviours

So the right acceptance stage is a **small, real** acceptance stage, not a fake “enterprise” stage full of ceremony.

### How acceptance grows later

As ADE grows, acceptance is where you add:

- browser acceptance tests
- API workflow/integration tests
- document processing happy-path tests
- worker/runtime contract tests

When those suites exist and become slow, **parallelize inside the acceptance stage**.
Do **not** create extra stages unless the feedback economics actually require it.

## Release stage

Purpose: deploy an accepted version to production without rebuilding it.

Current ADE release stage runs automatically on `main` after acceptance and is also exposed through `workflow_dispatch` for emergency redeploy with:

- input: `candidate_tag`

It then:

- resolves the exact accepted image digests from the annotated candidate tag metadata
- deploys those exact digests to the `production` Azure Container Apps environment
- smoke tests production immediately after deploy via `pnpm run test:smoke`
- on success, creates the matching immutable Git release tag:

```text
release-<utc-timestamp>-<sha12>
```

- uploads a `release-manifest.json` artifact
- if production smoke fails, attempts rollback to the most recent successful `release-*` tag and leaves the run failed

### Why release now deploys instead of only promoting

Because this repo now contains a concrete deployment target and deployment contract.

That is fine.

For ADE, the simplest correct implementation is now:

- prove the candidate is accepted
- deploy the same accepted images to production
- smoke test production immediately after deploy
- rollback automatically if that smoke fails

Do **not** change the stage structure when the app grows.
Just extend the acceptance contract as the product needs it.

## GitHub repository setup required

Create `.github/workflows/pipeline.yml` from the file in this directory.

Then configure the repo like this:

### 1. Acceptance and production environments

Create GitHub environments named:

```text
acceptance
production
```

Set these variables on both environments:

- `AZURE_CLIENT_ID`
- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_RESOURCE_GROUP`

If the registry is private, also set:

- `ADE_REGISTRY_SERVER`
- `ADE_REGISTRY_USERNAME`
- secret: `ADE_REGISTRY_PASSWORD`

If one `ADE_REGISTRY_*` value is set, all three must be set.

### 2. Azure authentication

The workflow uses `azure/login` with OIDC.

Configure Azure federated credentials for the GitHub environments instead of storing long-lived cloud secrets in the repository.

### 3. Container registry and workflow permissions

The workflow pushes candidate images to GitHub Container Registry (`ghcr.io`) using `GITHUB_TOKEN`. Permissions are granted per job with least privilege:

- `commit_verify`: `contents: read`
- `candidate_build_and_smoke`: `contents: read`
- `publish_candidate`: `contents: read`, `packages: write`
- `acceptance-stage`: `contents: write`, `id-token: write`
- `production-release`: `contents: write`, `id-token: write`
- `emergency-redeploy`: `contents: write`, `id-token: write`

### 4. Branching discipline

Use `main` as the only authoritative integration branch.

If you keep pull requests:

- keep them very short-lived
- merge quickly
- do not treat PR-only automation as the authoritative pipeline

## How to release

Normal release:

1. Push to `main`.
2. Wait for commit and acceptance to pass.
3. The workflow deploys the accepted candidate to production automatically.
4. If production smoke passes, the workflow creates the immutable release tag.

Emergency redeploy:

1. Find the accepted candidate tag (`candidate-<sha>`).
2. Go to **Actions**.
3. Run the **deployment-pipeline** workflow manually.
4. Enter the accepted `candidate_tag`.
5. The workflow redeploys that exact accepted candidate to production, smoke tests it, and creates a new release tag on success.

## Why there is no PR CI in this initial implementation

Because this pipeline is intentionally the **canonical Farley pipeline**, not a GitHub convenience layer.

Dave Farley’s point is that the **real, authoritative feedback** is on the integrated change set on trunk. Anything else is, at best, a helpful approximation.

If you want optional preflight automation later, add it later and keep it clearly non-authoritative.
Do not let it replace the real deployment pipeline on `main`.

## What to defer until the system matures

Keep these out of the authoritative pipeline for now unless they become truly necessary:

- CodeQL as a merge/release gate
- SAST/dependency scanning as a blocking gate
- performance/load stages
- browser matrix testing
- merge queue
- ephemeral preview environments
- deployment orchestration for infra you do not yet have

Add them only when the system’s actual risk profile demands them.

## Summary

This pipeline is intentionally opinionated.

It says:

- integrate to `main`
- get fast feedback on `main`
- produce immutable candidates once
- acceptance-test the same candidate in a real acceptance environment
- mark accepted candidates explicitly
- release the accepted candidate to production without rebuilding it
- keep the number of stages low
- keep every stage easy to explain

That is the simplest ADE pipeline that is faithful to Dave Farley’s model and still practical for the repo you have today.
