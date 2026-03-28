"""HTTP session agent used inside ADE sandbox containers."""

from __future__ import annotations

import argparse
from collections import deque
from dataclasses import asdict, dataclass
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import logging
import os
from pathlib import Path
import pty
import select
import shlex
import subprocess
import sys
import threading
import time
from typing import Any
from urllib.parse import parse_qs, unquote, urlparse

LOGGER = logging.getLogger("ade_engine.session_agent")
DEFAULT_EVENT_BUFFER_SIZE = 512
DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 9000
DEFAULT_WAIT_MS = 15_000


@dataclass(frozen=True)
class AgentEvent:
    seq: int
    time: int
    type: str
    payload: dict[str, Any]


@dataclass(frozen=True)
class InstalledConfigStatus:
    package_name: str
    version: str
    sha256: str
    fingerprint: str


@dataclass(frozen=True)
class TerminalStatus:
    open: bool
    cwd: str
    cols: int | None = None
    rows: int | None = None


@dataclass(frozen=True)
class RunStatus:
    active: bool
    input_path: str | None = None
    output_path: str | None = None
    pid: int | None = None


@dataclass(frozen=True)
class AgentStatus:
    workspace: str
    installed_config: InstalledConfigStatus | None
    terminal: TerminalStatus
    run: RunStatus
    earliest_seq: int
    latest_seq: int


@dataclass(frozen=True)
class EventPollState:
    needs_resync: bool
    events: list[AgentEvent]


@dataclass(frozen=True)
class PollEventsResult:
    needs_resync: bool
    events: list[AgentEvent]
    status: AgentStatus


class EventBuffer:
    def __init__(self, max_events: int) -> None:
        self._events: deque[AgentEvent] = deque(maxlen=max_events)
        self._next_seq = 1
        self._condition = threading.Condition()

    def append(self, event_type: str, payload: dict[str, Any]) -> AgentEvent:
        with self._condition:
            event = AgentEvent(
                seq=self._next_seq,
                time=int(time.time() * 1000),
                type=event_type,
                payload=payload,
            )
            self._next_seq += 1
            self._events.append(event)
            self._condition.notify_all()
            return event

    def snapshot(self) -> tuple[int, int]:
        with self._condition:
            return self._earliest_seq_locked(), self._latest_seq_locked()

    def poll(self, after: int, wait_ms: int) -> EventPollState:
        deadline = time.monotonic() + (max(wait_ms, 0) / 1000)

        with self._condition:
            while True:
                earliest_seq = self._earliest_seq_locked()

                if earliest_seq > 0 and after < earliest_seq - 1:
                    return EventPollState(
                        needs_resync=True,
                        events=[],
                    )

                ready_events = [event for event in self._events if event.seq > after]
                if ready_events:
                    return EventPollState(
                        needs_resync=False,
                        events=ready_events,
                    )

                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return EventPollState(
                        needs_resync=False,
                        events=[],
                    )

                self._condition.wait(timeout=remaining)

    def _earliest_seq_locked(self) -> int:
        return self._events[0].seq if self._events else 0

    def _latest_seq_locked(self) -> int:
        return self._events[-1].seq if self._events else 0


class SessionAgent:
    def __init__(
        self,
        workspace: Path,
        *,
        event_buffer_size: int = DEFAULT_EVENT_BUFFER_SIZE,
    ) -> None:
        self.workspace = workspace.resolve()
        self.inputs_dir = self.workspace / "inputs"
        self.outputs_dir = self.workspace / "outputs"
        self.wheels_dir = self.workspace / ".wheels"
        self.events = EventBuffer(event_buffer_size)
        self.installed_config: InstalledConfigStatus | None = None
        self._terminal_process: subprocess.Popen[bytes] | None = None
        self._terminal_master_fd: int | None = None
        self._terminal_rows: int | None = None
        self._terminal_cols: int | None = None
        self._run_process: subprocess.Popen[str] | None = None
        self._run_input_path: str | None = None
        self._run_output_path: str | None = None
        self._lock = threading.Lock()

        self.inputs_dir.mkdir(parents=True, exist_ok=True)
        self.outputs_dir.mkdir(parents=True, exist_ok=True)
        self.wheels_dir.mkdir(parents=True, exist_ok=True)
        self.events.append("session.ready", {"workspace": str(self.workspace)})

    def status(self) -> AgentStatus:
        earliest_seq, latest_seq = self.events.snapshot()
        with self._lock:
            terminal_status = self._terminal_status_locked()
            run_status = self._run_status_locked()
            return AgentStatus(
                workspace=str(self.workspace),
                installed_config=self.installed_config,
                terminal=terminal_status,
                run=run_status,
                earliest_seq=earliest_seq,
                latest_seq=latest_seq,
            )

    def poll_events(self, after: int, wait_ms: int) -> PollEventsResult:
        state = self.events.poll(after, wait_ms)
        return PollEventsResult(
            needs_resync=state.needs_resync,
            events=state.events,
            status=self.status(),
        )

    def install_config(
        self,
        wheel_bytes: bytes,
        *,
        package_name: str,
        version: str,
        sha256: str,
        wheel_filename: str,
    ) -> AgentStatus:
        self.events.append(
            "config.install.started",
            {"packageName": package_name, "version": version},
        )
        wheel_path = self._write_wheel(
            wheel_bytes,
            package_name,
            version,
            sha256,
            wheel_filename,
        )
        self._install_wheel(wheel_path)
        self.installed_config = InstalledConfigStatus(
            package_name=package_name,
            version=version,
            sha256=sha256,
            fingerprint=_config_fingerprint(package_name, version, sha256),
        )
        self.events.append(
            "config.install.completed",
            asdict(self.installed_config),
        )
        return self.status()

    def upload_file(self, relative_path: str, content: bytes) -> dict[str, Any]:
        target_path = self._resolve_workspace_path(relative_path)
        target_path.parent.mkdir(parents=True, exist_ok=True)
        target_path.write_bytes(content)
        return {
            "path": str(target_path.relative_to(self.workspace)),
            "size": len(content),
        }

    def download_file(self, relative_path: str) -> bytes:
        target_path = self._resolve_workspace_path(relative_path)
        if not target_path.is_file():
            raise FileNotFoundError(f"File not found: {relative_path}")
        return target_path.read_bytes()

    def rpc(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        if method == "terminal.open":
            return self._terminal_open(
                rows=int(params.get("rows", 24)),
                cols=int(params.get("cols", 80)),
            )
        if method == "terminal.input":
            return self._terminal_input(str(params.get("data", "")))
        if method == "terminal.resize":
            return self._terminal_resize(
                rows=int(params.get("rows", 24)),
                cols=int(params.get("cols", 80)),
            )
        if method == "terminal.close":
            return self._terminal_close()
        if method == "run.start":
            input_path = str(params.get("inputPath", "")).strip()
            output_dir = str(params.get("outputDir", "outputs")).strip() or "outputs"
            return self._run_start(input_path=input_path, output_dir=output_dir)
        if method == "run.cancel":
            return self._run_cancel()
        raise ValueError(f"Unsupported RPC method: {method}")

    def _write_wheel(
        self,
        wheel_bytes: bytes,
        package_name: str,
        version: str,
        sha256: str,
        wheel_filename: str,
    ) -> Path:
        actual_sha = _sha256_hex(wheel_bytes)
        if actual_sha != sha256:
            raise ValueError("Wheel sha256 does not match the uploaded content.")

        wheel_name = Path(wheel_filename).name.strip()
        if wheel_name == "":
            raise ValueError("A wheel filename is required.")
        if not wheel_name.endswith(".whl"):
            raise ValueError("Wheel filename must end with .whl.")
        wheel_path = self.wheels_dir / wheel_name
        wheel_path.write_bytes(wheel_bytes)
        return wheel_path

    def _install_wheel(self, wheel_path: Path) -> None:
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "pip",
                "install",
                "--no-deps",
                "--force-reinstall",
                str(wheel_path),
            ],
            capture_output=True,
            check=False,
            cwd=self.workspace,
            text=True,
        )
        if result.returncode != 0:
            stderr = result.stderr.strip()
            stdout = result.stdout.strip()
            message = stderr or stdout or "pip install failed"
            raise RuntimeError(message)

    def _resolve_workspace_path(self, relative_path: str) -> Path:
        candidate = relative_path.strip().lstrip("/")
        if candidate == "":
            raise ValueError("A relative workspace path is required.")
        target_path = (self.workspace / candidate).resolve()
        if target_path != self.workspace and self.workspace not in target_path.parents:
            raise ValueError("Path must stay inside the sandbox workspace.")
        return target_path

    def _terminal_open(self, *, rows: int, cols: int) -> dict[str, Any]:
        with self._lock:
            if self._terminal_process is not None and self._terminal_process.poll() is None:
                return asdict(self._terminal_status_locked())

            master_fd, slave_fd = pty.openpty()
            self._set_terminal_size(master_fd, rows, cols)
            process = subprocess.Popen(
                [os.environ.get("SHELL", "/bin/sh")],
                cwd=self.workspace,
                stdin=slave_fd,
                stdout=slave_fd,
                stderr=slave_fd,
                close_fds=True,
                start_new_session=True,
            )
            os.close(slave_fd)
            self._terminal_process = process
            self._terminal_master_fd = master_fd
            self._terminal_rows = rows
            self._terminal_cols = cols
            threading.Thread(
                target=self._read_terminal_output,
                name="ade-terminal-reader",
                daemon=True,
            ).start()

        self.events.append(
            "terminal.opened",
            {"rows": rows, "cols": cols, "cwd": str(self.workspace)},
        )
        return asdict(self.status().terminal)

    def _terminal_input(self, data: str) -> dict[str, Any]:
        with self._lock:
            if self._terminal_master_fd is None or self._terminal_process is None:
                raise ValueError("Terminal is not open.")
            os.write(self._terminal_master_fd, data.encode("utf-8"))
        return {"ok": True}

    def _terminal_resize(self, *, rows: int, cols: int) -> dict[str, Any]:
        with self._lock:
            if self._terminal_master_fd is None or self._terminal_process is None:
                raise ValueError("Terminal is not open.")
            self._set_terminal_size(self._terminal_master_fd, rows, cols)
            self._terminal_rows = rows
            self._terminal_cols = cols

        self.events.append("terminal.resized", {"rows": rows, "cols": cols})
        return asdict(self.status().terminal)

    def _terminal_close(self) -> dict[str, Any]:
        process: subprocess.Popen[bytes] | None
        with self._lock:
            process = self._terminal_process
            self._terminal_process = None
            master_fd = self._terminal_master_fd
            self._terminal_master_fd = None
            self._terminal_rows = None
            self._terminal_cols = None

        if master_fd is not None:
            try:
                os.close(master_fd)
            except OSError:
                pass

        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()

        self.events.append("terminal.closed", {})
        return {"ok": True}

    def _set_terminal_size(self, fd: int, rows: int, cols: int) -> None:
        import fcntl
        import struct
        import termios

        packed = struct.pack("HHHH", rows, cols, 0, 0)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, packed)

    def _read_terminal_output(self) -> None:
        while True:
            with self._lock:
                process = self._terminal_process
                master_fd = self._terminal_master_fd

            if process is None or master_fd is None:
                return

            try:
                ready, _, _ = select.select([master_fd], [], [], 0.25)
            except OSError:
                break
            if not ready:
                if process.poll() is not None:
                    break
                continue

            try:
                chunk = os.read(master_fd, 4096)
            except OSError:
                break

            if not chunk:
                break

            self.events.append(
                "terminal.output",
                {"data": chunk.decode("utf-8", errors="replace")},
            )

        exit_code = 0
        with self._lock:
            process = self._terminal_process
            self._terminal_process = None
            master_fd = self._terminal_master_fd
            self._terminal_master_fd = None
            self._terminal_rows = None
            self._terminal_cols = None

        if process is not None:
            exit_code = process.wait()

        if master_fd is not None:
            try:
                os.close(master_fd)
            except OSError:
                pass

        self.events.append("terminal.exited", {"exitCode": exit_code})

    def _run_start(self, *, input_path: str, output_dir: str) -> dict[str, Any]:
        if input_path == "":
            raise ValueError("inputPath is required.")

        input_file = self._resolve_workspace_path(input_path)
        if not input_file.is_file():
            raise FileNotFoundError(f"Input file not found: {input_path}")

        output_dir_path = self._resolve_workspace_path(output_dir)
        output_dir_path.mkdir(parents=True, exist_ok=True)
        output_path = output_dir_path / f"{input_file.stem}.normalized.xlsx"

        with self._lock:
            if self._run_process is not None and self._run_process.poll() is None:
                raise ValueError("An ADE run is already active.")

            process = self._start_run_process(input_file, output_dir_path)
            self._run_process = process
            self._run_input_path = str(input_file.relative_to(self.workspace))
            self._run_output_path = str(output_path.relative_to(self.workspace))

        self.events.append(
            "run.started",
            {
                "inputPath": self._run_input_path,
                "outputPath": self._run_output_path,
                "pid": process.pid,
            },
        )
        threading.Thread(
            target=self._read_run_output,
            name="ade-run-reader",
            daemon=True,
        ).start()
        return {
            "inputPath": self._run_input_path,
            "outputPath": self._run_output_path,
            "pid": process.pid,
        }

    def _start_run_process(
        self,
        input_file: Path,
        output_dir: Path,
    ) -> subprocess.Popen[str]:
        command = [
            "ade",
            "process",
            str(input_file),
            "--output-dir",
            str(output_dir),
        ]
        LOGGER.info("Starting ADE run: %s", shlex.join(command))
        return subprocess.Popen(
            command,
            cwd=self.workspace,
            env={**os.environ, "PYTHONUNBUFFERED": "1"},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )

    def _read_run_output(self) -> None:
        with self._lock:
            process = self._run_process
            input_path = self._run_input_path
            output_path = self._run_output_path

        if process is None:
            return

        def read_stream(stream_name: str, stream: Any) -> None:
            if stream is None:
                return
            for line in iter(stream.readline, ""):
                text = line.rstrip("\n")
                if text == "":
                    continue
                self.events.append(
                    "run.log",
                    {"stream": stream_name, "message": text},
                )

        stdout_thread = threading.Thread(
            target=read_stream,
            args=("stdout", process.stdout),
            daemon=True,
        )
        stderr_thread = threading.Thread(
            target=read_stream,
            args=("stderr", process.stderr),
            daemon=True,
        )
        stdout_thread.start()
        stderr_thread.start()
        exit_code = process.wait()
        stdout_thread.join(timeout=1)
        stderr_thread.join(timeout=1)

        with self._lock:
            self._run_process = None
            self._run_input_path = None
            self._run_output_path = None

        if exit_code == 0:
            self.events.append(
                "run.completed",
                {
                    "exitCode": exit_code,
                    "inputPath": input_path,
                    "outputPath": output_path,
                },
            )
        else:
            self.events.append(
                "run.failed",
                {
                    "exitCode": exit_code,
                    "inputPath": input_path,
                    "outputPath": output_path,
                },
            )

    def _run_cancel(self) -> dict[str, Any]:
        with self._lock:
            process = self._run_process
        if process is None or process.poll() is not None:
            return {"cancelled": False}

        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)
        self.events.append("run.cancelled", {"pid": process.pid})
        return {"cancelled": True}

    def _terminal_status_locked(self) -> TerminalStatus:
        terminal_open = self._terminal_process is not None and self._terminal_process.poll() is None
        return TerminalStatus(
            open=terminal_open,
            cwd=str(self.workspace),
            cols=self._terminal_cols if terminal_open else None,
            rows=self._terminal_rows if terminal_open else None,
        )

    def _run_status_locked(self) -> RunStatus:
        run_active = self._run_process is not None and self._run_process.poll() is None
        return RunStatus(
            active=run_active,
            input_path=self._run_input_path if run_active else None,
            output_path=self._run_output_path if run_active else None,
            pid=self._run_process.pid if run_active and self._run_process else None,
        )


def _sha256_hex(content: bytes) -> str:
    import hashlib

    return hashlib.sha256(content).hexdigest()


def _config_fingerprint(package_name: str, version: str, sha256: str) -> str:
    return f"{package_name}@{version}:{sha256}"


class SessionAgentHandler(BaseHTTPRequestHandler):
    server: "SessionAgentHttpServer"

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)

        if parsed.path == "/healthz":
            self._write_json(HTTPStatus.OK, {"status": "ok"})
            return

        if parsed.path == "/readyz":
            self._write_json(HTTPStatus.OK, {"status": "ready"})
            return

        if parsed.path == "/v1/status":
            self._write_json(HTTPStatus.OK, _serialize_status(self.server.agent.status()))
            return

        if parsed.path == "/v1/events":
            query = parse_qs(parsed.query)
            after = int(query.get("after", ["0"])[0] or 0)
            wait_ms = int(query.get("waitMs", [str(DEFAULT_WAIT_MS)])[0] or DEFAULT_WAIT_MS)
            result = self.server.agent.poll_events(after, wait_ms)
            self._write_json(HTTPStatus.OK, _serialize_poll_result(result))
            return

        if parsed.path.startswith("/v1/files/"):
            relative_path = unquote(parsed.path.removeprefix("/v1/files/"))
            try:
                content = self.server.agent.download_file(relative_path)
            except FileNotFoundError:
                self._write_json(HTTPStatus.NOT_FOUND, {"message": "File not found."})
                return
            except ValueError as error:
                self._write_json(HTTPStatus.BAD_REQUEST, {"message": str(error)})
                return

            self.send_response(HTTPStatus.OK)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            self.wfile.write(content)
            return

        self._write_json(HTTPStatus.NOT_FOUND, {"message": "Route not found."})

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        content_length = int(self.headers.get("Content-Length", "0") or 0)
        body = self.rfile.read(content_length)

        try:
            if parsed.path == "/v1/config/install":
                self._handle_install(body)
                return
            if parsed.path == "/v1/files":
                self._handle_upload(parsed.query, body)
                return
            if parsed.path == "/v1/rpc":
                self._handle_rpc(body)
                return
        except FileNotFoundError as error:
            self.server.agent.events.append("error", {"message": str(error)})
            self._write_json(HTTPStatus.NOT_FOUND, {"message": str(error)})
            return
        except ValueError as error:
            self.server.agent.events.append("error", {"message": str(error)})
            self._write_json(HTTPStatus.BAD_REQUEST, {"message": str(error)})
            return
        except Exception as error:  # noqa: BLE001
            LOGGER.exception("Session agent request failed.")
            self.server.agent.events.append("error", {"message": str(error)})
            self._write_json(HTTPStatus.INTERNAL_SERVER_ERROR, {"message": str(error)})
            return

        self._write_json(HTTPStatus.NOT_FOUND, {"message": "Route not found."})

    def log_message(self, format: str, *args: Any) -> None:
        LOGGER.info("%s - %s", self.address_string(), format % args)

    def _handle_install(self, body: bytes) -> None:
        package_name = (self.headers.get("X-ADE-Package") or "").strip()
        version = (self.headers.get("X-ADE-Version") or "").strip()
        sha256 = (self.headers.get("X-ADE-Sha256") or "").strip()
        wheel_filename = (self.headers.get("X-ADE-Wheel-Filename") or "").strip()
        if package_name == "" or version == "" or sha256 == "" or wheel_filename == "":
            raise ValueError(
                "X-ADE-Package, X-ADE-Version, X-ADE-Sha256, and X-ADE-Wheel-Filename are required."
            )

        status = self.server.agent.install_config(
            body,
            package_name=package_name,
            version=version,
            sha256=sha256,
            wheel_filename=wheel_filename,
        )
        self._write_json(HTTPStatus.OK, _serialize_status(status))

    def _handle_upload(self, query_string: str, body: bytes) -> None:
        query = parse_qs(query_string)
        relative_path = (query.get("path", [""])[0] or "").strip()
        if relative_path == "":
            raise ValueError("path is required.")
        result = self.server.agent.upload_file(relative_path, body)
        self._write_json(HTTPStatus.OK, result)

    def _handle_rpc(self, body: bytes) -> None:
        try:
            payload = json.loads(body.decode("utf-8") or "{}")
        except json.JSONDecodeError as error:
            raise ValueError("Request body must be valid JSON.") from error

        method = payload.get("method")
        params = payload.get("params", {})
        if not isinstance(method, str) or method.strip() == "":
            raise ValueError("method is required.")
        if not isinstance(params, dict):
            raise ValueError("params must be an object.")

        result = self.server.agent.rpc(method, params)
        self._write_json(HTTPStatus.OK, {"result": result})

    def _write_json(self, status: HTTPStatus, payload: dict[str, Any]) -> None:
        content = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)


class SessionAgentHttpServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(
        self,
        server_address: tuple[str, int],
        agent: SessionAgent,
    ) -> None:
        super().__init__(server_address, SessionAgentHandler)
        self.agent = agent


def _serialize_status(status: AgentStatus) -> dict[str, Any]:
    return {
        "workspace": status.workspace,
        "installedConfig": asdict(status.installed_config)
        if status.installed_config is not None
        else None,
        "terminal": asdict(status.terminal),
        "run": asdict(status.run),
        "earliestSeq": status.earliest_seq,
        "latestSeq": status.latest_seq,
    }


def _serialize_poll_result(result: PollEventsResult) -> dict[str, Any]:
    return {
        "needsResync": result.needs_resync,
        "events": [asdict(event) for event in result.events],
        "status": _serialize_status(result.status),
    }


def create_server(
    host: str,
    port: int,
    workspace: Path,
    *,
    event_buffer_size: int = DEFAULT_EVENT_BUFFER_SIZE,
) -> SessionAgentHttpServer:
    agent = SessionAgent(workspace, event_buffer_size=event_buffer_size)
    return SessionAgentHttpServer((host, port), agent)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="ade-sandbox-agent")
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--workspace", type=Path, default=Path("/workspace"))
    parser.add_argument(
        "--event-buffer-size",
        type=int,
        default=DEFAULT_EVENT_BUFFER_SIZE,
    )
    args = parser.parse_args(argv)

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )

    server = create_server(
        args.host,
        args.port,
        args.workspace,
        event_buffer_size=args.event_buffer_size,
    )
    LOGGER.info("Starting ADE sandbox agent on %s:%s", args.host, args.port)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        LOGGER.info("Stopping ADE sandbox agent.")
    finally:
        server.server_close()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
