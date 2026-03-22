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
- the **release stage** chooses a previously accepted version and promotes it without rebuilding it
- the same candidate is promoted forward; it is **not rebuilt for release**
- if the mainline build goes red, the team **stops the line** and fixes or reverts immediately

This repository is still small, so the pipeline is kept intentionally compact:

- **one canonical workflow file**
- **one commit-stage job**
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

### 3. Release means choosing an accepted version

A version is releasable only after:

- `commit-stage` passed on `main`
- `acceptance-stage` passed on `main`
- the workflow created an immutable Git tag of the form:

```text
candidate-<40-char-sha>
```

Releasing is then just:

- choose the accepted candidate SHA
- run the workflow manually
- approve the protected `production` environment
- let the workflow promote the exact same images and create a release tag

That is the Farley model: **build once, test once per stage, then promote**.

## What each stage does

## Commit stage

Purpose: reject bad changes quickly and produce an immutable release candidate.

Current ADE commit stage does the following:

- checks out the repo
- uses the repo-pinned **Node 22.12.0** and **Python 3.12** toolchain
- installs JavaScript dependencies with `pnpm install --frozen-lockfile`
- runs the repo-owned `pnpm run ci:commit` contract
- includes static analysis, typechecks, API unit tests, Python package builds, API packaging, and image creation
- measures and records the commit-stage wall-clock duration
- pushes immutable candidate image tags to GHCR:

```text
ghcr.io/<owner>/<repo>-web:sha-<commit-sha>
ghcr.io/<owner>/<repo>-api:sha-<commit-sha>
```

- uploads a `candidate-manifest.json` artifact for traceability
- carries the accepted digests forward into the annotated `candidate-<sha>` tag so release can resolve the candidate from Git alone

### What belongs here today

Exactly the checks that are already meaningful and fast in this repo:

- TypeScript compile/build checks
- API unit tests
- Python package builds
- image creation

### What does **not** belong here yet

Do **not** add these to the commit stage yet:

- browser end-to-end suites
- performance tests
- long-running integration environments
- heavyweight security scans that add material latency
- tool-driven “quality theatre” that is not yet part of your definition of deployable

If a check is not fast, deterministic, and clearly valuable right now, it stays out.

## Acceptance stage

Purpose: deploy the exact release candidate and prove it works in a production-shaped run.

Current ADE acceptance stage does the following:

- pulls the exact immutable candidate images created in commit stage
- injects those exact digest refs into the existing compose-based startup path
- runs the existing production-shaped smoke test via the repo-owned `pnpm run smoke` contract:

```sh
pnpm run smoke
```

- if that passes, creates the immutable accepted-candidate tag:

```text
candidate-<commit-sha>
```

### Why the acceptance stage is small right now

Because ADE is still early.

At the moment, the repository has:

- a small Fastify API
- a small Vite/React web app
- a production-shaped local start path
- a real smoke check for `/` and `/api/healthz`

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

Purpose: choose an accepted version and promote it without rebuilding.

Current ADE release stage is triggered manually with:

- `workflow_dispatch`
- input: `candidate_tag`

It then:

- verifies that the requested accepted candidate tag exists
- resolves the exact accepted image digests from the annotated candidate tag metadata
- waits for approval on the protected `production` environment
- promotes the exact immutable candidate images to immutable release tags:

```text
release-<utc-timestamp>-<sha12>
```

- creates the matching Git release tag:

```text
release-<utc-timestamp>-<sha12>
```

- uploads a `release-manifest.json` artifact

### Why release currently promotes instead of deploying

Because this repo does not yet contain a real production deployment target.

That is fine.

For an early rewrite repository, the simplest correct implementation is:

- prove the candidate is accepted
- make the chosen release explicit and immutable
- do not introduce fake infrastructure complexity before it exists

When a real deploy target exists, keep this exact release model and add one step:

- deploy the promoted release images to production
- smoke test production immediately after deploy

Do **not** change the stage structure when you do that.
Just extend the release stage.

## GitHub repository setup required

Create `.github/workflows/pipeline.yml` from the file in this directory.

Then configure the repo like this:

### 1. Production environment

Create a GitHub environment named:

```text
production
```

Configure it with:

- **Required reviewers**
- **Prevent self-review**
- optionally a small wait timer if you want extra friction before production promotion

### 2. Container registry permissions

The workflow pushes images to GitHub Container Registry (`ghcr.io`) using `GITHUB_TOKEN`. Permissions are granted per job with least privilege:

- `commit-stage`: `contents: read`, `packages: write`
- `acceptance-stage`: `contents: read`, `packages: read`
- `mark-accepted-candidate`: `contents: write`
- `release-stage`: `contents: write`, `packages: write`

### 3. Branching discipline

Use `main` as the only authoritative integration branch.

If you keep pull requests:

- keep them very short-lived
- merge quickly
- do not treat PR-only automation as the authoritative pipeline

## How to release

1. Wait for a `main` pipeline run to pass commit and acceptance.
2. Find the accepted candidate tag (the workflow will have created a tag `candidate-<sha>`).
3. Go to **Actions**.
4. Run the **deployment-pipeline** workflow manually.
5. Enter the accepted `candidate_tag`.
6. Approve the `production` environment when prompted.
7. The workflow will create immutable release tags for the images and Git history.

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
- acceptance-test the same candidate
- mark accepted candidates explicitly
- release by selecting an accepted candidate and promoting it without rebuilding
- keep the number of stages low
- keep every stage easy to explain

That is the simplest ADE pipeline that is faithful to Dave Farley’s model and still practical for the repo you have today.
