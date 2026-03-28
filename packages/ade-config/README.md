# ADE Config

`ade-config` is the installed ADE product package.

It exposes the `ade` command, installs a pinned `ade-engine`, and provides the
business rule modules that the engine loads from `fields/`, `row_detectors/`,
and `hooks/`.

Install locally:

```sh
uv sync --directory packages/ade-config
```

Run a file:

```sh
uv run --directory packages/ade-config --no-sync ade process ./path/to/file.xlsx --output-dir ./out
```

Published install:

```sh
pip install ade-config
ade process ./path/to/file.xlsx --output-dir ./out
```

Rule modules define `register(config)` and can register detectors, transforms,
validators, and hooks with optional `priority=`. Lower `priority` runs first.

This scaffold still stops at the current engine boundary before spreadsheet
parsing is implemented.
