"""Runtime entrypoint for the ADE engine."""

from pathlib import Path

from ade_engine.types import AdeConfig


def run(*, config: AdeConfig, input_path: Path, output_dir: Path) -> None:
    """Accept the installed config package and stop at the parsing boundary."""

    if not input_path.exists():
        raise NotImplementedError(f"Input path does not exist: '{input_path}'.")

    source_kind = "file" if input_path.is_file() else "directory" if input_path.is_dir() else None
    if source_kind is None:
        raise NotImplementedError(
            f"Input path must be a file or directory: '{input_path}'.",
        )

    raise NotImplementedError(
        "ADE parsing is not implemented yet. "
        f"The config '{config.name}' reached ade-engine successfully for "
        f"{source_kind} input '{input_path}' with output directory '{output_dir}'."
    )
