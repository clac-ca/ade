"""CLI for the ADE engine runtime."""

import argparse
from importlib.metadata import PackageNotFoundError, version as read_version
from importlib.util import find_spec
from pathlib import Path
import sys

from ade_engine.config import load_config
from ade_engine.runner import process

CONFIG_IMPORT_PACKAGE = "ade_config"
CONFIG_DISTRIBUTION = "ade-config"
ENGINE_DISTRIBUTION = "ade-engine"


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="ade", description="ADE CLI.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("version", help="Print the installed ADE versions.")

    process_parser = subparsers.add_parser(
        "process", help="Process an input file or directory."
    )
    process_parser.add_argument(
        "input_path", type=Path, help="Input file or directory."
    )
    process_parser.add_argument(
        "--output-dir",
        type=Path,
        required=True,
        help="Target output directory.",
    )

    return parser


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)

    if args.command == "version":
        try:
            engine_version = read_version(ENGINE_DISTRIBUTION)
        except PackageNotFoundError:
            engine_version = "unknown"
        print(f"{ENGINE_DISTRIBUTION} {engine_version}")

        if find_spec(CONFIG_IMPORT_PACKAGE) is None:
            print(f"{CONFIG_DISTRIBUTION} not installed")
            return 0

        try:
            config_version = read_version(CONFIG_DISTRIBUTION)
        except PackageNotFoundError:
            print(f"{CONFIG_DISTRIBUTION} not installed")
        else:
            print(f"{CONFIG_DISTRIBUTION} {config_version}")
        return 0

    if find_spec(CONFIG_IMPORT_PACKAGE) is None:
        print(
            "ADE config package is not installed. Install 'ade-config' and try again.",
            file=sys.stderr,
        )
        return 1

    config = load_config(CONFIG_IMPORT_PACKAGE, name=CONFIG_DISTRIBUTION)

    try:
        process(
            config=config,
            input_path=args.input_path,
            output_dir=args.output_dir,
        )
    except (FileNotFoundError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1

    return 0
