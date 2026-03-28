# Python Packages

This repo keeps the Python surface intentionally small:

- `packages/ade-engine` is the runtime library and CLI
- `packages/ade-config` is the installed business-rules package

## Package Roles

- `ade-engine` owns the CLI, rule discovery, ordered registration, the runtime config model, and the engine boundary.
- `ade-config` owns business rules under `fields/`, `row_detectors/`, and `hooks/`.

## Rule Modules

Each rule module defines `register(config)`.

```python
def register(config):
    config.field("email", priority=200)
    config.detector("email", score_email_header, priority=200)
    config.transform("email", normalize_email, priority=200)
    config.validator("email", validate_email, priority=300)
```

Rules are discovered from:

- `fields/`
- `row_detectors/`
- `hooks/`

Lower `priority` runs first. Equal priorities keep registration order.
Declared field order becomes the normalized workbook column order.

## Tooling

The standard Python stack in this repo is:

- `uv` for environments, installs, and builds
- `pytest` for tests
- `ruff` for linting and formatting
- `argparse` for the small engine CLI

## Common Commands

```sh
uv sync --directory packages/ade-engine --group dev
uv run --directory packages/ade-engine pytest
uv run --directory packages/ade-engine ruff check .
uv run --directory packages/ade-engine ruff format --check .
uv build --directory packages/ade-engine

uv sync --directory packages/ade-config --group dev
uv run --directory packages/ade-config pytest
uv run --directory packages/ade-config ruff check .
uv run --directory packages/ade-config ruff format --check .
uv build --directory packages/ade-config
```

Repo-level shortcuts:

```sh
pnpm lint:python
pnpm format:python
pnpm format:python:check
pnpm test:python
pnpm package:python
```

## Testing Strategy

- Keep unit tests close to the package that owns the behavior.
- Use package-local fixture packages for loader tests.
- Use direct function tests for the CLI instead of framework-heavy harnesses.
- Add real spreadsheet fixtures later for parser and workbook integration tests.

## High-Quality Python Code

- No import-time work except definitions.
- Prefer stdlib-first solutions unless a dependency clearly pays for itself.
- Prefer built-in exceptions over custom hierarchies unless a custom type clearly adds meaning.
- Keep public APIs small and explicit.
- Represent each concept once.
- Make important product contracts explicit, especially field order.
- Keep package READMEs short and audience-specific.
- Keep contributor detail in repo docs, not package landing pages.
- Make tests own their fixtures instead of borrowing state from another package.
