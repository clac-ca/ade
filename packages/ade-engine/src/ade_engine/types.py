"""Shared types for the ADE engine runtime boundary."""

from typing import Protocol


class AdeConfig(Protocol):
    """Minimal business-package contract passed into the engine."""

    name: str
