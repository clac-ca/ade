"""Typer CLI for the installed ADE product package."""

from importlib.metadata import version as read_version
from pathlib import Path

import typer

from ade_config.config import CONFIG
from ade_engine import run

app = typer.Typer(
    help="ADE CLI.",
    no_args_is_help=True,
)


@app.callback()
def main() -> None:
    """ADE CLI."""


@app.command("version")
def version_command() -> None:
    """Print the installed ade-config version."""
    typer.echo(read_version("ade-config"))


@app.command("process")
def process_command(
    input_path: Path = typer.Argument(
        ...,
        exists=True,
        file_okay=True,
        dir_okay=True,
        readable=True,
        help="Input file or directory.",
    ),
    output_dir: Path = typer.Option(..., "--output-dir", help="Target output directory."),
) -> None:
    """Hand the installed ADE config package to the engine runtime."""

    try:
        run(config=CONFIG, input_path=input_path, output_dir=output_dir)
    except NotImplementedError as error:
        typer.echo(str(error), err=True)
        raise typer.Exit(code=1) from error
