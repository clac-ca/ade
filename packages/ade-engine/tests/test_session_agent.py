import hashlib
import json
from pathlib import Path
import subprocess
import sys
import threading
from types import MethodType
from urllib import error as urllib_error
from urllib import request as urllib_request

import pytest

from ade_engine.session_agent import create_server


def _json_request(
    base_url: str,
    path: str,
    *,
    method: str = "GET",
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
) -> tuple[int, dict]:
    request = urllib_request.Request(
        f"{base_url}{path}",
        data=body,
        headers=headers or {},
        method=method,
    )

    try:
        with urllib_request.urlopen(request, timeout=5) as response:
            return response.status, json.loads(response.read().decode("utf-8"))
    except urllib_error.HTTPError as response:
        return response.code, json.loads(response.read().decode("utf-8"))


def _bytes_request(base_url: str, path: str) -> tuple[int, bytes]:
    with urllib_request.urlopen(f"{base_url}{path}", timeout=5) as response:
        return response.status, response.read()


def _poll_events(base_url: str, *, after: int = 0, wait_ms: int = 0) -> dict:
    status_code, payload = _json_request(
        base_url,
        f"/v1/events?after={after}&waitMs={wait_ms}",
    )
    assert status_code == 200
    return payload


@pytest.fixture
def agent_server(tmp_path: Path):
    server = create_server("127.0.0.1", 0, tmp_path / "workspace", event_buffer_size=3)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base_url = f"http://127.0.0.1:{server.server_address[1]}"

    try:
        yield server, base_url
    finally:
        server.shutdown()
        thread.join(timeout=5)
        server.server_close()


def test_health_and_status_routes_report_ready(agent_server) -> None:
    server, base_url = agent_server

    status_code, health = _json_request(base_url, "/healthz")
    assert status_code == 200
    assert health == {"status": "ok"}

    status_code, ready = _json_request(base_url, "/readyz")
    assert status_code == 200
    assert ready == {"status": "ready"}

    status_code, status = _json_request(base_url, "/v1/status")
    assert status_code == 200
    assert status["workspace"] == str(server.agent.workspace)
    assert status["installedConfig"] is None
    assert status["terminal"]["open"] is False
    assert status["run"]["active"] is False


def test_config_install_reports_fingerprint_and_events(agent_server) -> None:
    server, base_url = agent_server
    wheel_bytes = b"fake-wheel"
    sha256 = hashlib.sha256(wheel_bytes).hexdigest()

    def fake_install(self, wheel_path: Path) -> None:
        assert wheel_path.read_bytes() == wheel_bytes
        assert wheel_path.name == "ade_config-0.1.0-py3-none-any.whl"

    server.agent._install_wheel = MethodType(fake_install, server.agent)

    status_code, status = _json_request(
        base_url,
        "/v1/config/install",
        method="POST",
        body=wheel_bytes,
        headers={
            "X-ADE-Package": "ade-config",
            "X-ADE-Version": "0.1.0",
            "X-ADE-Sha256": sha256,
            "X-ADE-Wheel-Filename": "ade_config-0.1.0-py3-none-any.whl",
        },
    )

    assert status_code == 200
    assert status["installedConfig"] == {
        "package_name": "ade-config",
        "version": "0.1.0",
        "sha256": sha256,
        "fingerprint": f"ade-config@0.1.0:{sha256}",
    }

    events = _poll_events(base_url)
    assert [event["type"] for event in events["events"]] == [
        "session.ready",
        "config.install.started",
        "config.install.completed",
    ]


def test_files_round_trip_inside_workspace(agent_server) -> None:
    _, base_url = agent_server

    status_code, result = _json_request(
        base_url,
        "/v1/files?path=inputs/example.csv",
        method="POST",
        body=b"Name,Email\nAlice,alice@example.com\n",
    )
    assert status_code == 200
    assert result["path"] == "inputs/example.csv"

    status_code, content = _bytes_request(base_url, "/v1/files/inputs/example.csv")
    assert status_code == 200
    assert content == b"Name,Email\nAlice,alice@example.com\n"


def test_terminal_open_input_and_close_emit_events(agent_server) -> None:
    _, base_url = agent_server

    status_code, opened = _json_request(
        base_url,
        "/v1/rpc",
        method="POST",
        body=json.dumps(
            {"method": "terminal.open", "params": {"rows": 24, "cols": 80}}
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    assert status_code == 200
    assert opened["result"]["open"] is True

    status_code, reopened = _json_request(
        base_url,
        "/v1/rpc",
        method="POST",
        body=json.dumps(
            {"method": "terminal.open", "params": {"rows": 30, "cols": 100}}
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    assert status_code == 200
    assert reopened["result"] == opened["result"]

    status_code, _ = _json_request(
        base_url,
        "/v1/rpc",
        method="POST",
        body=json.dumps(
            {"method": "terminal.input", "params": {"data": "echo hello-agent\\n"}}
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    assert status_code == 200

    events = _poll_events(base_url, wait_ms=1000)
    assert [event["type"] for event in events["events"]].count("terminal.opened") == 1
    assert any(
        event["type"] == "terminal.output"
        and "hello-agent" in event["payload"]["data"]
        for event in events["events"]
    )

    status_code, closed = _json_request(
        base_url,
        "/v1/rpc",
        method="POST",
        body=json.dumps({"method": "terminal.close", "params": {}}).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    assert status_code == 200
    assert closed["result"]["ok"] is True


def test_run_start_emits_logs_and_writes_output(agent_server) -> None:
    server, base_url = agent_server

    def fake_start_run_process(
        self,
        input_file: Path,
        output_dir: Path,
    ) -> subprocess.Popen[str]:
        command = [
            sys.executable,
            "-c",
            (
                "from pathlib import Path; import sys; "
                "input_path = Path(sys.argv[1]); output_dir = Path(sys.argv[2]); "
                "print('processing', input_path.name); "
                "(output_dir / f'{input_path.stem}.normalized.xlsx').write_bytes(b'xlsx'); "
            ),
            str(input_file),
            str(output_dir),
        ]
        return subprocess.Popen(
            command,
            cwd=self.workspace,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )

    server.agent._start_run_process = MethodType(fake_start_run_process, server.agent)

    _json_request(
        base_url,
        "/v1/files?path=inputs/sample.csv",
        method="POST",
        body=b"Name,Email\nAlice,alice@example.com\n",
    )

    status_code, started = _json_request(
        base_url,
        "/v1/rpc",
        method="POST",
        body=json.dumps(
            {
                "method": "run.start",
                "params": {"inputPath": "inputs/sample.csv", "outputDir": "outputs"},
            }
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    assert status_code == 200
    assert started["result"]["outputPath"] == "outputs/sample.normalized.xlsx"

    collected_events: list[dict] = []
    latest_seq = 0
    for _ in range(10):
        batch = _poll_events(base_url, after=latest_seq, wait_ms=250)
        collected_events.extend(batch["events"])
        if batch["events"]:
            latest_seq = batch["events"][-1]["seq"]
        if any(event["type"] == "run.completed" for event in collected_events):
            break

    assert any(event["type"] == "run.started" for event in collected_events)
    assert any(
        event["type"] == "run.log"
        and event["payload"]["message"] == "processing sample.csv"
        for event in collected_events
    )
    assert any(event["type"] == "run.completed" for event in collected_events)

    status_code, output = _bytes_request(
        base_url,
        "/v1/files/outputs/sample.normalized.xlsx",
    )
    assert status_code == 200
    assert output == b"xlsx"

    status_code, status = _json_request(base_url, "/v1/status")
    assert status_code == 200
    assert status["run"] == {
        "active": False,
        "input_path": None,
        "output_path": None,
        "pid": None,
    }


def test_event_poll_reports_resync_when_cursor_is_too_old(agent_server) -> None:
    server, base_url = agent_server
    server.agent.events.append("one", {})
    server.agent.events.append("two", {})
    server.agent.events.append("three", {})

    events = _poll_events(base_url, after=0)
    assert events["needsResync"] is True
    assert events["events"] == []
