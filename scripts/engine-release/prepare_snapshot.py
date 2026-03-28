from __future__ import annotations

import os
import re
from pathlib import Path


def _require_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"{name} is required")
    return value


def _replace_exactly_one(
    text: str,
    *,
    pattern: str,
    replacement: str,
    label: str,
) -> str:
    matches = re.findall(pattern, text, flags=re.MULTILINE)
    if len(matches) != 1:
        raise ValueError(
            f"expected exactly one match for {label}, found {len(matches)}"
        )
    result, count = re.subn(pattern, replacement, text, count=1, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f"failed to replace {label}")
    return result


def prepare_snapshot(
    *, root: Path, release_version: str, release_tag: str, repository: str
) -> None:
    engine_pyproject = root / "packages/ade-engine/pyproject.toml"
    config_pyproject = root / "packages/ade-config/pyproject.toml"
    engine_dependency = (
        f"ade-engine @ git+https://github.com/{repository}.git@{release_tag}"
        "#subdirectory=packages/ade-engine"
    )

    engine_text = engine_pyproject.read_text()
    config_text = config_pyproject.read_text()

    engine_text = _replace_exactly_one(
        engine_text,
        pattern=r'^version = "[^"]+"$',
        replacement=f'version = "{release_version}"',
        label="ade-engine version",
    )
    config_text = _replace_exactly_one(
        config_text,
        pattern=r'^version = "[^"]+"$',
        replacement=f'version = "{release_version}"',
        label="ade-config version",
    )
    config_text = _replace_exactly_one(
        config_text,
        pattern=r"^dependencies = \[.*\]$",
        replacement=f'dependencies = ["{engine_dependency}"]',
        label="ade-config dependency",
    )

    engine_pyproject.write_text(engine_text)
    config_pyproject.write_text(config_text)


def main() -> None:
    root = Path(__file__).resolve().parents[2]
    prepare_snapshot(
        root=root,
        release_version=_require_env("RELEASE_VERSION"),
        release_tag=_require_env("RELEASE_TAG"),
        repository=_require_env("GITHUB_REPOSITORY"),
    )


if __name__ == "__main__":
    main()
