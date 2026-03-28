# ADE Config

Installed ADE business-rules package. Installing it also makes the `ade`
command available through `ade-engine`.

Install:

```sh
pip install "ade-config @ git+https://github.com/clac-ca/ade.git@ade-py-v2026.3.28.42#subdirectory=packages/ade-config"
```

Replace `ade-py-v2026.3.28.42` with the published Git tag you want to install.

Run:

```sh
ade process ./path/to/file.xlsx --output-dir ./out
```

Local development:

```sh
uv sync --directory packages/ade-config --group dev
uv run --directory packages/ade-config pytest
```

Spreadsheet parsing is not implemented yet; `ade process` currently stops at
the engine boundary.
