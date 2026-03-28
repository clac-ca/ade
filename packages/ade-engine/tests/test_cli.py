from pathlib import Path

import pytest

import ade_engine.cli as cli
from ade_engine import EngineConfig, FieldRules
from ade_engine.runner import RunResult


def test_version_command_reports_engine_and_config_versions(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setattr(
        cli,
        "read_version",
        lambda distribution: {
            "ade-engine": "0.1.0",
            "ade-config": "0.2.0",
        }[distribution],
    )
    monkeypatch.setattr(cli, "find_spec", lambda package: object())
    monkeypatch.setattr(
        cli,
        "load_config",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("version should not load config")
        ),
    )

    result = cli.main(["version"])
    captured = capsys.readouterr()

    assert result == 0
    assert captured.out.splitlines() == [
        "ade-engine 0.1.0",
        "ade-config 0.2.0",
    ]


def test_version_command_reports_missing_config(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setattr(cli, "read_version", lambda distribution: "0.1.0")
    monkeypatch.setattr(cli, "find_spec", lambda package: None)

    result = cli.main(["version"])
    captured = capsys.readouterr()

    assert result == 0
    assert captured.out.splitlines() == [
        "ade-engine 0.1.0",
        "ade-config not installed",
    ]


def test_process_command_passes_the_installed_config_to_the_engine(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")
    output_dir = tmp_path / "out"
    captured: dict[str, object] = {}
    config = EngineConfig(
        name="ade-config",
        fields={
            "full_name": FieldRules(),
            "email": FieldRules(),
        },
    )

    def fake_process(
        *, config: object, input_path: Path, output_dir: Path
    ) -> RunResult:
        captured["config"] = config
        captured["input_path"] = input_path
        captured["output_dir"] = output_dir
        return RunResult(output_path=output_dir / "input.normalized.xlsx")

    monkeypatch.setattr(cli, "find_spec", lambda package: object())
    monkeypatch.setattr(cli, "load_config", lambda package, *, name: config)
    monkeypatch.setattr(cli, "process", fake_process)

    result = cli.main(["process", str(input_path), "--output-dir", str(output_dir)])

    assert result == 0
    assert captured == {
        "config": config,
        "input_path": input_path,
        "output_dir": output_dir,
    }
    assert config.name == "ade-config"
    assert list(config.fields) == ["full_name", "email"]


def test_process_command_reports_missing_config(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")
    monkeypatch.setattr(cli, "find_spec", lambda package: None)

    result = cli.main(
        ["process", str(input_path), "--output-dir", str(tmp_path / "out")]
    )
    captured = capsys.readouterr()

    assert result == 1
    assert (
        captured.err.strip()
        == "ADE config package is not installed. Install 'ade-config' and try again."
    )


def test_process_command_returns_exit_code_one_for_expected_failures(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")
    monkeypatch.setattr(cli, "find_spec", lambda package: object())
    monkeypatch.setattr(
        cli, "load_config", lambda package, *, name: EngineConfig(name="ade-config")
    )

    def fake_process(
        *, config: object, input_path: Path, output_dir: Path
    ) -> RunResult:
        raise ValueError("unsupported input")

    monkeypatch.setattr(cli, "process", fake_process)

    result = cli.main(
        ["process", str(input_path), "--output-dir", str(tmp_path / "out")]
    )
    captured = capsys.readouterr()

    assert result == 1
    assert "unsupported input" in captured.err


def test_process_command_propagates_unexpected_failures(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")
    monkeypatch.setattr(cli, "find_spec", lambda package: object())
    monkeypatch.setattr(
        cli, "load_config", lambda package, *, name: EngineConfig(name="ade-config")
    )

    def fake_process(
        *, config: object, input_path: Path, output_dir: Path
    ) -> RunResult:
        raise RuntimeError("unexpected failure")

    monkeypatch.setattr(cli, "process", fake_process)

    with pytest.raises(RuntimeError, match="unexpected failure"):
        cli.main(["process", str(input_path), "--output-dir", str(tmp_path / "out")])
