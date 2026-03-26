from dataclasses import dataclass
from pathlib import Path

import pytest

from ade_engine import run


@dataclass(frozen=True)
class ExampleConfig:
    name: str = "example-config"


def test_run_rejects_missing_input_path(tmp_path: Path) -> None:
    with pytest.raises(NotImplementedError, match="Input path does not exist"):
        run(
            config=ExampleConfig(),
            input_path=tmp_path / "missing.xlsx",
            output_dir=tmp_path / "out",
        )


def test_run_reports_file_boundary(tmp_path: Path) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")

    with pytest.raises(NotImplementedError, match="example-config"):
        run(
            config=ExampleConfig(),
            input_path=input_path,
            output_dir=tmp_path / "out",
        )


def test_run_reports_directory_boundary(tmp_path: Path) -> None:
    input_dir = tmp_path / "input-dir"
    input_dir.mkdir()

    with pytest.raises(NotImplementedError, match="directory input"):
        run(
            config=ExampleConfig(),
            input_path=input_dir,
            output_dir=tmp_path / "out",
        )
