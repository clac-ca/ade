# ADE Engine Tests

These tests prove the engine platform: config loading, processing behavior,
format support, and public runtime boundaries.

Prefer small pure tests first. Use scenario-style tests when the real engine
behavior matters, such as reading an input file and writing a normalized
workbook.

- Keep fixtures small and local to `packages/ade-engine/tests`.
- Use plain pytest, `tmp_path`, and direct assertions.
- Prefer built-in exceptions and direct function calls over custom test helpers.
- Inspect the output workbook only when workbook behavior is the subject.

Example:

```python
import importlib
from pathlib import Path

from ade_engine import load_config, process
from ade_engine.testing import read_rows


def test_process_writes_normalized_workbook(monkeypatch, tmp_path):
    fixtures_dir = Path(__file__).parent / "fixtures"
    monkeypatch.syspath_prepend(str(fixtures_dir))
    importlib.invalidate_caches()

    config = load_config("fixture_config", name="fixture-config")
    input_path = tmp_path / "input.csv"
    input_path.write_text("Name,Email Address\nAlice,Alice@Example.com\n")

    result = process(config=config, input_path=input_path, output_dir=tmp_path / "out")

    assert read_rows(result.output_path) == [
        ["full_name", "email"],
        ["Alice", "alice@example.com"],
    ]
```
