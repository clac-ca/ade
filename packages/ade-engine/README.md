# ADE Engine

`ade-engine` is ADE's runtime library.

It owns the execution boundary between the installed business package and the
file-processing engine. The installed product is `ade-config`, which depends on
this package and calls `ade_engine.run(...)`.

This scaffold is intentionally minimal. It defines a tiny typed handoff
contract and a `run(...)` entrypoint, but it does not implement parsing yet.
The runtime currently validates that the input path exists and identifies
whether it is a file or directory before stopping at the deliberate
not-implemented boundary.

`ade-engine` is not the user-facing CLI package. Users install `ade-config` and
run `ade`.

## Build

Build the source distribution and wheel:

```sh
uv build --directory packages/ade-engine
```

For local ADE product development, `packages/ade-config` resolves this package
through its `[tool.uv.sources]` override in editable mode.
