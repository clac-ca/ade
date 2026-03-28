"""Public config model and loader for ADE engine packages."""

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

Rule = Callable[..., Any]


@dataclass
class FieldRules:
    detectors: list[Rule] = field(default_factory=list)
    transforms: list[Rule] = field(default_factory=list)
    validators: list[Rule] = field(default_factory=list)


@dataclass
class EngineConfig:
    name: str
    fields: dict[str, FieldRules] = field(default_factory=dict)
    row_detectors: dict[str, list[Rule]] = field(
        default_factory=lambda: {"header": [], "data": []},
    )
    hooks: dict[str, list[Rule]] = field(default_factory=dict)


def load_config(package_name: str, *, name: str | None = None) -> EngineConfig:
    from ade_engine.loader import load_engine_config

    return load_engine_config(package_name, name=name or package_name)
