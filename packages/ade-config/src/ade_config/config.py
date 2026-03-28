"""Installed ADE business configuration."""

from ade_engine import Config, load_config

CONFIG: Config = load_config("ade_config", name="ade-config")
