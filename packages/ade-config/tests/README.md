# ADE Config Tests

These tests prove business logic. They assume `ade-engine` works and focus on
whether this config produces the expected normalized output.

Use the local `ade_run` fixture from `conftest.py`, plain `assert`, and small
scenario inputs under `tests/scenarios/`.

- Prefer `.csv` for most business-rule scenarios.
- Use readable row assertions first.
- Keep business expectations explicit, including the declared output field order.
- Open the output workbook only when workbook behavior is the point of the test.

Example:

```python
from pathlib import Path

from ade_engine.testing import read_rows

SCENARIOS_DIR = Path(__file__).parent / "scenarios"


def test_email_cleanup(ade_run):
    result = ade_run(SCENARIOS_DIR / "email_cleanup" / "input.csv")

    assert read_rows(result.output_path) == [
        ["full_name", "email"],
        ["Alice Smith", "alice@example.com"],
    ]
```
