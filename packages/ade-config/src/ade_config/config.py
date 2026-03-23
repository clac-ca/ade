"""Installed ADE business configuration."""

from dataclasses import dataclass

from ade_engine.types import AdeConfig


@dataclass(frozen=True)
class Config:
    """Minimal ADE business package identity passed into the engine."""

    name: str = "ade-config"


CONFIG: AdeConfig = Config()
