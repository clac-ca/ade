from pathlib import Path

from ade_engine.testing import open_workbook

SCENARIOS_DIR = Path(__file__).parent / "scenarios"


def test_summary_hook_adds_a_summary_sheet(ade_run) -> None:
    result = ade_run(SCENARIOS_DIR / "email_cleanup" / "input.csv")

    workbook = open_workbook(result.output_path)

    assert workbook.sheetnames == ["Normalized Output", "Summary"]
    assert workbook["Summary"]["A1"].value == "Summary for Normalized Output"
