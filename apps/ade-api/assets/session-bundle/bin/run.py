import json
import subprocess
import sys
from pathlib import Path

from ade_engine import load_config
from ade_engine.runner import process


def access_request(name: str, payload: dict) -> dict:
    return payload[name]


def curl_command(blob: dict) -> list[str]:
    command = [
        "curl",
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--request",
        blob["method"],
    ]
    for name, value in blob["headers"].items():
        command.extend(["--header", f"{name}: {value}"])
    return command


def download(blob: dict, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [*curl_command(blob), "--output", str(destination), blob["url"]],
        check=True,
    )


def upload(blob: dict, source: Path) -> None:
    subprocess.run(
        [*curl_command(blob), "--upload-file", str(source), blob["url"]],
        check=True,
    )


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: run.py <request-path> <result-path>")

    request_path = Path(sys.argv[1])
    result_path = Path(sys.argv[2])
    payload = json.loads(request_path.read_text())
    input_path = Path(payload["localInputPath"])
    output_dir = Path(payload["localOutputDir"])

    print(f"Downloading input to {input_path}", flush=True)
    download(access_request("inputAccess", payload), input_path)

    print(f"Processing workbook into {output_dir}", flush=True)
    config = load_config("ade_config", name="ade-config")
    result = process(config=config, input_path=input_path, output_dir=output_dir)

    print(f"Uploading output from {result.output_path}", flush=True)
    upload(access_request("outputAccess", payload), result.output_path)

    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.write_text(
        json.dumps(
            {
                "outputPath": payload["outputPath"],
                "validationIssues": [
                    {
                        "rowIndex": issue.row_index,
                        "field": issue.field,
                        "message": issue.message,
                    }
                    for issue in result.validation_issues
                ],
            },
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
