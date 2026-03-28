from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).resolve().parents[1] / "prepare_snapshot.py"
SPEC = importlib.util.spec_from_file_location("prepare_snapshot", MODULE_PATH)
assert SPEC is not None
assert SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def _write_pyprojects(
    root: Path, *, engine_version: str, config_version: str, dependency_line: str
) -> None:
    (root / "packages/ade-engine").mkdir(parents=True)
    (root / "packages/ade-config").mkdir(parents=True)
    (root / "packages/ade-engine/pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "ade-engine"',
                f'version = "{engine_version}"',
                'dependencies = ["openpyxl>=3.1,<4"]',
                "",
            ]
        )
    )
    (root / "packages/ade-config/pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "ade-config"',
                f'version = "{config_version}"',
                dependency_line,
                "",
                "[tool.uv.sources]",
                'ade-engine = { path = "../ade-engine", editable = true }',
                "",
            ]
        )
    )


def test_prepare_snapshot_rewrites_versions_and_dependency(tmp_path: Path) -> None:
    _write_pyprojects(
        tmp_path,
        engine_version="0.0.0",
        config_version="0.0.0",
        dependency_line='dependencies = ["ade-engine"]',
    )

    MODULE.prepare_snapshot(
        root=tmp_path,
        release_version="2026.3.28.42",
        release_tag="ade-py-v2026.3.28.42",
        repository="clac-ca/ade",
    )

    engine_text = (tmp_path / "packages/ade-engine/pyproject.toml").read_text()
    config_text = (tmp_path / "packages/ade-config/pyproject.toml").read_text()

    assert 'version = "2026.3.28.42"' in engine_text
    assert 'version = "2026.3.28.42"' in config_text
    assert (
        'dependencies = ["ade-engine @ git+https://github.com/clac-ca/ade.git@'
        'ade-py-v2026.3.28.42#subdirectory=packages/ade-engine"]'
    ) in config_text
    assert 'ade-engine = { path = "../ade-engine", editable = true }' in config_text


def test_prepare_snapshot_fails_when_engine_version_is_missing(tmp_path: Path) -> None:
    _write_pyprojects(
        tmp_path,
        engine_version="0.0.0",
        config_version="0.0.0",
        dependency_line='dependencies = ["ade-engine"]',
    )
    engine_pyproject = tmp_path / "packages/ade-engine/pyproject.toml"
    engine_pyproject.write_text(
        engine_pyproject.read_text().replace('version = "0.0.0"\n', "")
    )

    with pytest.raises(ValueError, match="ade-engine version"):
        MODULE.prepare_snapshot(
            root=tmp_path,
            release_version="2026.3.28.42",
            release_tag="ade-py-v2026.3.28.42",
            repository="clac-ca/ade",
        )


def test_prepare_snapshot_fails_when_config_dependency_is_missing(
    tmp_path: Path,
) -> None:
    _write_pyprojects(
        tmp_path,
        engine_version="0.0.0",
        config_version="0.0.0",
        dependency_line="dependencies = []",
    )
    config_pyproject = tmp_path / "packages/ade-config/pyproject.toml"
    config_pyproject.write_text(
        config_pyproject.read_text().replace("dependencies = []\n", "")
    )

    with pytest.raises(ValueError, match="ade-config dependency"):
        MODULE.prepare_snapshot(
            root=tmp_path,
            release_version="2026.3.28.42",
            release_tag="ade-py-v2026.3.28.42",
            repository="clac-ca/ade",
        )


def test_prepare_snapshot_fails_when_config_version_is_duplicated(
    tmp_path: Path,
) -> None:
    _write_pyprojects(
        tmp_path,
        engine_version="0.0.0",
        config_version="0.0.0",
        dependency_line='dependencies = ["ade-engine"]',
    )
    config_pyproject = tmp_path / "packages/ade-config/pyproject.toml"
    config_text = config_pyproject.read_text()
    config_pyproject.write_text(
        config_text.replace(
            'version = "0.0.0"\n',
            'version = "0.0.0"\nversion = "0.0.0"\n',
        )
    )

    with pytest.raises(ValueError, match="ade-config version"):
        MODULE.prepare_snapshot(
            root=tmp_path,
            release_version="2026.3.28.42",
            release_tag="ade-py-v2026.3.28.42",
            repository="clac-ca/ade",
        )
