# ADE Engine

ADE runtime library and CLI package. Users typically get it by installing
`ade-config`, which provides the business rules and pulls `ade-engine` in as a
dependency.

Published releases are cut from Git tags in the monorepo. End users normally
install `ade-config`, not `ade-engine`, directly. Engine release tags use the
`ade-engine-v...` prefix.

Local development:

```sh
uv sync --directory packages/ade-engine --group dev
uv run --directory packages/ade-engine pytest
uv run --directory packages/ade-engine ade version
uv build --directory packages/ade-engine
```
