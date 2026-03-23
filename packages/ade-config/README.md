# ADE Config

`ade-config` is the installed ADE product package.

It exposes the `ade` command, depends on a pinned `ade-engine`, and owns the
business-side configuration that gets passed into the engine runtime.

This scaffold intentionally stops before parsing logic. The CLI proves the
handoff into `ade-engine`, then exits at a clear not-yet-implemented boundary.

## Local Usage

Create the local package environment first:

```sh
uv sync --directory packages/ade-config
```

Show the installed package version:

```sh
uv run --directory packages/ade-config --no-sync ade version
```

Run the product CLI through the engine boundary:

```sh
uv run --directory packages/ade-config --no-sync ade process ./path/to/file.xls --output-dir ./output
uv run --directory packages/ade-config --no-sync ade process ./path/to/input-directory --output-dir ./output/batch
```

Run with `python -m`:

```sh
uv run --directory packages/ade-config --no-sync python -m ade_config version
```

## Local Development

For published installs, `ade-config` depends on the pinned `ade-engine`
package version declared in `pyproject.toml`.

Inside this repo, the local source override in `[tool.uv.sources]` makes
`uv sync --directory packages/ade-config` resolve `../ade-engine` directly in
editable mode during development. After that, use
`uv run --directory packages/ade-config --no-sync ...` for local commands.

## Public Shape

For installed usage, the intended public command surface is:

```sh
pip install ade-config

ade process data/samples/CaressantWRH_251130__ORIGINAL.xls \
  --output-dir output

ade process data/samples \
  --output-dir output/batch
```

The CLI accepts either a single file or a directory as the input path and
passes that through to `ade-engine`.

## Build

Build the source distribution and wheel:

```sh
uv build --directory packages/ade-config
```
