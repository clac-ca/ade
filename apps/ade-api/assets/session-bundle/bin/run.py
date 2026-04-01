import json
import shutil
import sys
from pathlib import Path
from urllib.request import Request, urlopen

from ade_engine import load_config
from ade_engine.runner import process


def access_request(name: str, payload: dict) -> dict:
    return payload[name]


def download(blob: dict, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    request = Request(blob["url"], headers=blob["headers"], method=blob["method"])
    with urlopen(request) as response, destination.open("wb") as handle:
        shutil.copyfileobj(response, handle)


def upload(blob: dict, source: Path) -> None:
    request = Request(
        blob["url"],
        data=source.read_bytes(),
        headers=blob["headers"],
        method=blob["method"],
    )
    with urlopen(request) as response:
        response.read()


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
