from pathlib import Path

import pytest

from ade_engine import load_config, process


@pytest.fixture
def ade_run(tmp_path: Path):
    config = load_config("ade_config", name="ade-config")

    def run_case(input_path: Path):
        output_dir = tmp_path / "out"
        output_dir.mkdir(exist_ok=True)
        return process(config=config, input_path=input_path, output_dir=output_dir)

    return run_case
