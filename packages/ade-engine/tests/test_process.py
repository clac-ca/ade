from pathlib import Path
import importlib
import sys

import pytest
from openpyxl import Workbook

from ade_engine import load_config, process
from ade_engine.testing import open_workbook, read_rows

FIXTURES_DIR = Path(__file__).parent / "fixtures"


def _clear_package(package_name: str) -> None:
    for module_name in list(sys.modules):
        if module_name == package_name or module_name.startswith(f"{package_name}."):
            sys.modules.pop(module_name)


def _load_fixture_config(monkeypatch: pytest.MonkeyPatch):
    package_name = "fixture_config"
    monkeypatch.syspath_prepend(str(FIXTURES_DIR))
    importlib.invalidate_caches()
    _clear_package(package_name)
    return load_config(package_name, name="fixture-config")


def test_process_rejects_missing_input_path(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = _load_fixture_config(monkeypatch)

    with pytest.raises(FileNotFoundError, match="Input path does not exist"):
        process(
            config=config,
            input_path=tmp_path / "missing.csv",
            output_dir=tmp_path / "out",
        )


def test_process_writes_a_normalized_workbook_from_csv(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = _load_fixture_config(monkeypatch)
    input_path = tmp_path / "contacts.csv"
    input_path.write_text(
        "Name,Email Address\n Alice   Smith , Alice@Example.com \nBob Jones,bob@example.com\n",
        encoding="utf-8",
    )

    result = process(config=config, input_path=input_path, output_dir=tmp_path / "out")

    assert result.output_path.name == "contacts.normalized.xlsx"
    assert result.validation_issues == []
    assert read_rows(result.output_path) == [
        ["full_name", "email"],
        ["Alice Smith", "alice@example.com"],
        ["Bob Jones", "bob@example.com"],
    ]

    workbook = open_workbook(result.output_path)
    assert workbook.sheetnames == ["Normalized Output", "Summary"]
    assert workbook["Summary"]["A1"].value == "Summary for Normalized Output"


def test_process_writes_a_normalized_workbook_from_xlsx(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = _load_fixture_config(monkeypatch)
    input_path = tmp_path / "contacts.xlsx"
    workbook = Workbook()
    worksheet = workbook.active
    worksheet.append(["Employee Name", "Email"])
    worksheet.append([" Alice   Smith ", "Alice@Example.com "])
    workbook.save(input_path)

    result = process(config=config, input_path=input_path, output_dir=tmp_path / "out")

    assert read_rows(result.output_path) == [
        ["full_name", "email"],
        ["Alice Smith", "alice@example.com"],
    ]


def test_process_reports_validation_issues(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = _load_fixture_config(monkeypatch)
    input_path = tmp_path / "contacts.csv"
    input_path.write_text(
        "Name,Email Address\nCasey Example,not-an-email\n",
        encoding="utf-8",
    )

    result = process(config=config, input_path=input_path, output_dir=tmp_path / "out")

    assert [
        (issue.row_index, issue.field, issue.message)
        for issue in result.validation_issues
    ] == [(2, "email", "Invalid email")]
    assert read_rows(result.output_path) == [
        ["full_name", "email"],
        ["Casey Example", "not-an-email"],
    ]


def test_process_chooses_the_best_global_column_matches(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = _load_fixture_config(monkeypatch)
    input_path = tmp_path / "ambiguous.csv"
    input_path.write_text(
        "Name,Email Address,Email\nAlice,A@x.com,wrong@example.com\n",
        encoding="utf-8",
    )

    result = process(config=config, input_path=input_path, output_dir=tmp_path / "out")

    assert read_rows(result.output_path) == [
        ["full_name", "email"],
        ["Alice", "a@x.com"],
    ]


def test_process_rejects_directory_input(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = _load_fixture_config(monkeypatch)
    input_dir = tmp_path / "input-dir"
    input_dir.mkdir()

    with pytest.raises(ValueError, match="Directory input"):
        process(config=config, input_path=input_dir, output_dir=tmp_path / "out")


def test_process_rejects_unsupported_file_types(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = _load_fixture_config(monkeypatch)
    input_path = tmp_path / "contacts.txt"
    input_path.write_text("Name,Email\nAlice,alice@example.com\n", encoding="utf-8")

    with pytest.raises(ValueError, match="Unsupported input file type"):
        process(config=config, input_path=input_path, output_dir=tmp_path / "out")


def test_process_requires_a_header_row(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = _load_fixture_config(monkeypatch)
    input_path = tmp_path / "contacts.csv"
    input_path.write_text(
        "Alice,alice@example.com\nBob,bob@example.com\n",
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="Unable to detect a header row"):
        process(config=config, input_path=input_path, output_dir=tmp_path / "out")
