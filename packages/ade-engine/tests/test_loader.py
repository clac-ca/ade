from dataclasses import dataclass
from pathlib import Path
import importlib
import sys

import pytest

from ade_engine import load_config

FIXTURES_DIR = Path(__file__).parent / "fixtures"


@dataclass(frozen=True)
class Row:
    values: list[object]


@dataclass(frozen=True)
class Column:
    header: str | None
    sample_values: list[object]


def _clear_package(package_name: str) -> None:
    for module_name in list(sys.modules):
        if module_name == package_name or module_name.startswith(f"{package_name}."):
            sys.modules.pop(module_name)


def _load_fixture_package(
    monkeypatch: pytest.MonkeyPatch, package_name: str, *, name: str | None = None
):
    monkeypatch.syspath_prepend(str(FIXTURES_DIR))
    importlib.invalidate_caches()
    _clear_package(package_name)
    return load_config(package_name, name=name)


def test_load_config_discovers_modules_and_orders_rules(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    config = _load_fixture_package(
        monkeypatch,
        "fixture_config",
        name="fixture-config",
    )

    assert config.name == "fixture-config"
    assert list(config.fields) == ["full_name", "email"]
    assert [fn.__name__ for fn in config.fields["full_name"].detectors] == [
        "score_full_name"
    ]
    assert [fn.__name__ for fn in config.fields["email"].detectors] == [
        "score_email_header",
        "score_email_values",
    ]
    assert [fn.__name__ for fn in config.fields["email"].transforms] == [
        "strip_whitespace",
        "lowercase",
    ]
    assert [fn.__name__ for fn in config.fields["email"].validators] == [
        "require_at_symbol"
    ]
    assert [fn.__name__ for fn in config.row_detectors["data"]] == ["score_data"]
    assert [fn.__name__ for fn in config.row_detectors["header"]] == ["score_header"]
    assert [fn.__name__ for fn in config.hooks["on_table_written"]] == [
        "rename_output_sheet",
        "add_summary_sheet",
    ]

    row = Row(values=["Full Name", "Email Address"])
    column = Column(
        header="Email Address", sample_values=["A@Example.com", "B@Example.com"]
    )

    assert [fn(row) for fn in config.row_detectors["header"]] == [1.0]
    assert [fn(row) for fn in config.row_detectors["data"]] == [0.7]
    assert [fn(column) for fn in config.fields["email"].detectors] == [1.0, 0.7]

    value = " A@Example.com "
    for fn in config.fields["email"].transforms:
        value = fn(value)
    assert value == "a@example.com"


def test_load_config_requires_register_function(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    with pytest.raises(
        TypeError,
        match=r"Module 'fixture_missing_register\.hooks\.bad' must define register\(config\)",
    ):
        _load_fixture_package(monkeypatch, "fixture_missing_register")


def test_load_config_requires_explicit_field_declaration(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    with pytest.raises(
        ValueError,
        match=(
            "Field 'email' must be declared with config.field\\(\\.\\.\\.\\) before "
            "registering rules."
        ),
    ):
        _load_fixture_package(monkeypatch, "fixture_undeclared_field")


def test_load_config_rejects_duplicate_field_declarations(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    with pytest.raises(ValueError, match="Field 'email' is already declared."):
        _load_fixture_package(monkeypatch, "fixture_duplicate_field")
