from pathlib import Path

from typer.testing import CliRunner

import ade_config.cli as cli


runner = CliRunner()


def test_version_command_reports_installed_version(monkeypatch) -> None:
    monkeypatch.setattr(cli, "read_version", lambda _: "0.1.0")

    result = runner.invoke(cli.app, ["version"])

    assert result.exit_code == 0
    assert result.stdout.strip() == "0.1.0"


def test_process_command_passes_the_installed_config_to_the_engine(
    monkeypatch, tmp_path: Path
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")
    output_dir = tmp_path / "out"
    captured: dict[str, object] = {}

    def fake_run(*, config: object, input_path: Path, output_dir: Path) -> None:
        captured["config"] = config
        captured["input_path"] = input_path
        captured["output_dir"] = output_dir

    monkeypatch.setattr(cli, "run", fake_run)

    result = runner.invoke(
        cli.app,
        ["process", str(input_path), "--output-dir", str(output_dir)],
    )

    assert result.exit_code == 0
    assert captured == {
        "config": cli.CONFIG,
        "input_path": input_path,
        "output_dir": output_dir,
    }
    assert cli.CONFIG.name == "ade-config"
    assert sorted(cli.CONFIG.fields) == ["email", "full_name"]


def test_process_command_returns_exit_code_one_for_engine_boundaries(
    monkeypatch, tmp_path: Path
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")

    def fake_run(*, config: object, input_path: Path, output_dir: Path) -> None:
        raise NotImplementedError("engine boundary not implemented")

    monkeypatch.setattr(cli, "run", fake_run)

    result = runner.invoke(
        cli.app,
        ["process", str(input_path), "--output-dir", str(tmp_path / "out")],
    )

    assert result.exit_code == 1
    assert "engine boundary not implemented" in result.stderr
