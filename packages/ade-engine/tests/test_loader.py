from dataclasses import dataclass
from pathlib import Path
import importlib
import shutil
import sys

import pytest

from ade_engine import load_config

PACKAGES_DIR = Path(__file__).resolve().parents[2]
CONFIG_SRC_DIR = PACKAGES_DIR / "ade-config" / "src" / "ade_config"


@dataclass(frozen=True)
class Row:
    values: list[object]


@dataclass(frozen=True)
class Column:
    header: str | None
    sample_values: list[object]


def _copy_demo_package(tmp_path: Path, package_name: str) -> Path:
    src_dir = tmp_path / "src"
    shutil.copytree(CONFIG_SRC_DIR, src_dir / package_name)
    return src_dir


def _clear_package(package_name: str) -> None:
    for module_name in list(sys.modules):
        if module_name == package_name or module_name.startswith(f"{package_name}."):
            sys.modules.pop(module_name)


def test_load_config_discovers_modules_and_orders_rules(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    package_name = "ade_config_demo_success"
    src_dir = _copy_demo_package(tmp_path, package_name)

    monkeypatch.syspath_prepend(str(src_dir))
    importlib.invalidate_caches()
    _clear_package(package_name)

    config = load_config(package_name, name="ade-config-demo-success")

    assert config.name == "ade-config-demo-success"
    assert sorted(config.fields) == ["email", "full_name"]
    assert config.loaded_modules == [
        f"{package_name}.fields.email",
        f"{package_name}.fields.full_name",
        f"{package_name}.row_detectors.data",
        f"{package_name}.row_detectors.header",
        f"{package_name}.hooks.on_table_written",
    ]
    assert [fn.__name__ for fn in config.fields["email"].detectors] == [
        "score_email_header",
        "score_email_values",
    ]
    assert [fn.__name__ for fn in config.fields["email"].transforms] == [
        "strip_whitespace",
        "lowercase",
    ]
    assert [fn.__name__ for fn in config.fields["email"].validators] == ["require_at_symbol"]
    assert [fn.__name__ for fn in config.row_detectors["data"]] == ["score_data"]
    assert [fn.__name__ for fn in config.row_detectors["header"]] == ["score_header"]
    assert [fn.__name__ for fn in config.hooks["on_table_written"]] == [
        "log_table_written",
        "append_summary",
    ]

    row = Row(values=["Full Name", "Email Address"])
    column = Column(header="Email Address", sample_values=["A@Example.com", "B@Example.com"])

    assert [fn(row) for fn in config.row_detectors["header"]] == [1.0]
    assert [fn(row) for fn in config.row_detectors["data"]] == [0.7]
    assert [fn(column) for fn in config.fields["email"].detectors] == [1.0, 0.7]

    value = " A@Example.com "
    for fn in config.fields["email"].transforms:
        value = fn(value)
    assert value == "a@example.com"


def test_load_config_requires_register_function(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    package_name = "ade_config_demo_missing_register"
    src_dir = _copy_demo_package(tmp_path, package_name)
    bad_module = src_dir / package_name / "hooks" / "bad.py"
    bad_module.write_text("def noop():\n    return None\n")

    monkeypatch.syspath_prepend(str(src_dir))
    importlib.invalidate_caches()
    _clear_package(package_name)

    with pytest.raises(TypeError, match=rf"Module '{package_name}\.hooks\.bad' must define register\(config\)"):
        load_config(package_name)
