from pathlib import Path

import ade_engine.cli as cli
from ade_engine import EngineConfig, FieldRules


def test_version_command_reports_engine_and_config_versions(
    monkeypatch, capsys
) -> None:
    monkeypatch.setattr(
        cli,
        "_read_installed_version",
        lambda distribution: {
            "ade-engine": "0.1.0",
            "ade-config": "0.2.0",
        }.get(distribution),
    )
    monkeypatch.setattr(cli, "_config_is_installed", lambda: True)
    monkeypatch.setattr(
        cli,
        "_load_installed_config",
        lambda: (_ for _ in ()).throw(AssertionError("version should not load config")),
    )

    result = cli.main(["version"])
    captured = capsys.readouterr()

    assert result == 0
    assert captured.out.splitlines() == [
        "ade-engine 0.1.0",
        "ade-config 0.2.0",
    ]


def test_version_command_reports_missing_config(monkeypatch, capsys) -> None:
    monkeypatch.setattr(
        cli,
        "_read_installed_version",
        lambda distribution: "0.1.0" if distribution == "ade-engine" else None,
    )
    monkeypatch.setattr(cli, "_config_is_installed", lambda: False)

    result = cli.main(["version"])
    captured = capsys.readouterr()

    assert result == 0
    assert captured.out.splitlines() == [
        "ade-engine 0.1.0",
        "ade-config not installed",
    ]


def test_process_command_passes_the_installed_config_to_the_engine(
    monkeypatch, tmp_path: Path
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")
    output_dir = tmp_path / "out"
    captured: dict[str, object] = {}
    config = EngineConfig(
        name="ade-config",
        fields={
            "email": FieldRules(),
            "full_name": FieldRules(),
        },
    )

    def fake_run(*, config: object, input_path: Path, output_dir: Path) -> None:
        captured["config"] = config
        captured["input_path"] = input_path
        captured["output_dir"] = output_dir

    monkeypatch.setattr(cli, "_config_is_installed", lambda: True)
    monkeypatch.setattr(cli, "_load_installed_config", lambda: config)
    monkeypatch.setattr(cli, "run", fake_run)

    result = cli.main(["process", str(input_path), "--output-dir", str(output_dir)])

    assert result == 0
    assert captured == {
        "config": config,
        "input_path": input_path,
        "output_dir": output_dir,
    }
    assert config.name == "ade-config"
    assert sorted(config.fields) == ["email", "full_name"]


def test_process_command_reports_missing_config(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")
    monkeypatch.setattr(cli, "_config_is_installed", lambda: False)

    result = cli.main(
        ["process", str(input_path), "--output-dir", str(tmp_path / "out")]
    )
    captured = capsys.readouterr()

    assert result == 1
    assert captured.err.strip() == cli._missing_config_message()


def test_process_command_returns_exit_code_one_for_engine_boundaries(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    input_path = tmp_path / "input.xlsx"
    input_path.write_text("spreadsheet")
    monkeypatch.setattr(cli, "_config_is_installed", lambda: True)
    monkeypatch.setattr(
        cli, "_load_installed_config", lambda: EngineConfig(name="ade-config")
    )

    def fake_run(*, config: object, input_path: Path, output_dir: Path) -> None:
        raise NotImplementedError("engine boundary not implemented")

    monkeypatch.setattr(cli, "run", fake_run)

    result = cli.main(
        ["process", str(input_path), "--output-dir", str(tmp_path / "out")]
    )
    captured = capsys.readouterr()

    assert result == 1
    assert "engine boundary not implemented" in captured.err
