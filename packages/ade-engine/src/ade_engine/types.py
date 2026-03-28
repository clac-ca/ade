"""Shared types for the ADE engine runtime boundary."""

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any, Protocol

Rule = Callable[..., Any]


class AdeConfig(Protocol):
    """Minimal business-package contract passed into the engine."""

    name: str


@dataclass
class FieldRules:
    detectors: list[Rule] = field(default_factory=list)
    transforms: list[Rule] = field(default_factory=list)
    validators: list[Rule] = field(default_factory=list)


@dataclass
class Config:
    name: str = ""
    fields: dict[str, FieldRules] = field(default_factory=dict)
    row_detectors: dict[str, list[Rule]] = field(
        default_factory=lambda: {"header": [], "data": []},
    )
    hooks: dict[str, list[Rule]] = field(default_factory=dict)
    loaded_modules: list[str] = field(default_factory=list)
