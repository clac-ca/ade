"""CLI for the ADE engine runtime."""

import argparse
from importlib.metadata import PackageNotFoundError, version as read_version
from importlib.util import find_spec
from pathlib import Path
import sys

from ade_engine.config import load_config
from ade_engine.runner import run

CONFIG_IMPORT_PACKAGE = "ade_config"
CONFIG_DISTRIBUTION = "ade-config"
ENGINE_DISTRIBUTION = "ade-engine"


def _existing_path(value: str) -> Path:
    path = Path(value)
    if not path.exists():
        raise argparse.ArgumentTypeError(f"Input path does not exist: '{path}'.")
    return path


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="ade", description="ADE CLI.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("version", help="Print the installed ADE versions.")

    process_parser = subparsers.add_parser(
        "process", help="Process an input file or directory."
    )
    process_parser.add_argument(
        "input_path", type=_existing_path, help="Input file or directory."
    )
    process_parser.add_argument(
        "--output-dir",
        type=Path,
        required=True,
        help="Target output directory.",
    )

    return parser


def _config_is_installed() -> bool:
    return find_spec(CONFIG_IMPORT_PACKAGE) is not None


def _read_installed_version(distribution: str) -> str | None:
    try:
        return read_version(distribution)
    except PackageNotFoundError:
        return None


def _load_installed_config():
    return load_config(CONFIG_IMPORT_PACKAGE, name=CONFIG_DISTRIBUTION)


def _missing_config_message() -> str:
    return "ADE config package is not installed. Install 'ade-config' and try again."


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)

    if args.command == "version":
        engine_version = _read_installed_version(ENGINE_DISTRIBUTION) or "unknown"
        print(f"{ENGINE_DISTRIBUTION} {engine_version}")

        config_version = _read_installed_version(CONFIG_DISTRIBUTION)
        if config_version is None or not _config_is_installed():
            print(f"{CONFIG_DISTRIBUTION} not installed")
        else:
            print(f"{CONFIG_DISTRIBUTION} {config_version}")
        return 0

    if not _config_is_installed():
        print(_missing_config_message(), file=sys.stderr)
        return 1

    try:
        run(
            config=_load_installed_config(),
            input_path=args.input_path,
            output_dir=args.output_dir,
        )
    except NotImplementedError as error:
        print(str(error), file=sys.stderr)
        return 1

    return 0
