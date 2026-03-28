from pathlib import Path

from ade_engine.testing import read_rows

SCENARIOS_DIR = Path(__file__).parent / "scenarios"


def test_email_cleanup_normalizes_case_and_whitespace(ade_run) -> None:
    result = ade_run(SCENARIOS_DIR / "email_cleanup" / "input.csv")

    assert read_rows(result.output_path) == [
        ["full_name", "email"],
        ["Alice Smith", "alice@example.com"],
        ["Bob Jones", "bob@example.com"],
    ]
    assert result.validation_issues == []


def test_invalid_email_reports_a_validation_issue(ade_run) -> None:
    result = ade_run(SCENARIOS_DIR / "invalid_email" / "input.csv")

    assert read_rows(result.output_path) == [
        ["full_name", "email"],
        ["Casey Example", "not-an-email"],
    ]
    assert [
        (issue.row_index, issue.field, issue.message)
        for issue in result.validation_issues
    ] == [(2, "email", "Invalid email")]
