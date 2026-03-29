"""Local Azure-style Python session pool emulator for ADE development."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import UTC, datetime
from email.parser import BytesParser
from email.policy import default as email_policy
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
from typing import Any
from urllib.parse import parse_qs, unquote, urlparse
import uuid

LOGGER = logging.getLogger("ade.local.sessionpool")
DEFAULT_EXECUTION_TIMEOUT_SECONDS = 220
DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 9000


@dataclass(frozen=True)
class PythonSession:
    data_dir: Path
    venv_dir: Path


@dataclass(frozen=True)
class CommandResult:
    stdout: str
    stderr: str
    exit_code: int
    execution_time_in_milliseconds: int


class SessionPoolEmulator:
    def __init__(self, workspace_root: Path) -> None:
        self.workspace_root = workspace_root.resolve()
        self.sessions_root = self.workspace_root / "sessions"
        self.mnt_root = _resolve_mnt_root(self.workspace_root)
        self.mnt_data_path = self.mnt_root / "data"
        self.sessions_root.mkdir(parents=True, exist_ok=True)
        self._execution_lock = threading.Lock()

    def health(self) -> dict[str, str]:
        return {"status": "ok"}

    def upload_file(
        self,
        *,
        identifier: str,
        filename: str,
        content: bytes,
    ) -> dict[str, Any]:
        data_dir = self._job_session(identifier).data_dir
        target = self._resolve_file_path(data_dir, filename)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        return self._file_metadata(target, data_dir)

    def list_files(self, *, identifier: str) -> dict[str, Any]:
        data_dir = self._job_session(identifier).data_dir
        files = [
            self._file_metadata(path, data_dir)
            for path in sorted(path for path in data_dir.rglob("*") if path.is_file())
        ]
        return {"value": files}

    def download_file(self, *, identifier: str, filename: str) -> tuple[str, bytes]:
        data_dir = self._job_session(identifier).data_dir
        path = self._resolve_file_path(data_dir, filename)
        if not path.is_file():
            raise FileNotFoundError(f"File not found: {filename}")
        return "application/octet-stream", path.read_bytes()

    def execute(
        self,
        *,
        identifier: str,
        code: str,
        timeout_seconds: int = DEFAULT_EXECUTION_TIMEOUT_SECONDS,
    ) -> dict[str, Any]:
        session = self._job_session(identifier)
        result = self._run_python_code(
            session,
            code,
            timeout_seconds=min(timeout_seconds, DEFAULT_EXECUTION_TIMEOUT_SECONDS),
        )
        payload = {
            "status": "Succeeded" if result.exit_code == 0 else "Failed",
            "result": {
                "stdout": result.stdout,
                "stderr": result.stderr,
                "executionTimeInMilliseconds": result.execution_time_in_milliseconds,
            },
        }
        return payload

    def _job_session(self, identifier: str) -> PythonSession:
        session_root = self.sessions_root / identifier
        data_dir = session_root / "mnt-data"
        venv_dir = session_root / "venv"
        data_dir.mkdir(parents=True, exist_ok=True)
        if not venv_dir.exists():
            subprocess.run(
                [sys.executable, "-m", "venv", str(venv_dir)],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        return PythonSession(data_dir=data_dir, venv_dir=venv_dir)

    def _run_python_code(
        self,
        session: PythonSession,
        code: str,
        timeout_seconds: int,
    ) -> CommandResult:
        command = [
            str(_python_executable(session.venv_dir)),
            "-c",
            _rewrite_mnt_path(self.mnt_data_path, code),
        ]
        started_at = time.perf_counter()
        with self._execution_lock:
            _point_mnt_data(self.mnt_root, session.data_dir)
            completed = subprocess.run(
                command,
                cwd=session.data_dir,
                text=True,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout_seconds,
                check=False,
            )
        return CommandResult(
            stdout=completed.stdout,
            stderr=completed.stderr,
            exit_code=completed.returncode,
            execution_time_in_milliseconds=int(
                (time.perf_counter() - started_at) * 1000
            ),
        )

    def _resolve_file_path(self, data_dir: Path, filename: str) -> Path:
        candidate = filename.strip().lstrip("/")
        if candidate == "":
            raise ValueError("A filename is required.")
        path = (data_dir / candidate).resolve()
        if path != data_dir and data_dir not in path.parents:
            raise ValueError("Filename must stay inside the session data directory.")
        return path

    def _file_metadata(self, path: Path, data_dir: Path) -> dict[str, Any]:
        stat = path.stat()
        relative_path = path.relative_to(data_dir)
        directory = relative_path.parent.as_posix()
        return {
            "directory": "." if directory == "" else directory,
            "lastModifiedAt": datetime.fromtimestamp(stat.st_mtime, UTC).isoformat(),
            "name": relative_path.name,
            "sizeInBytes": stat.st_size,
            "type": "file",
        }


class SessionPoolRequestHandler(BaseHTTPRequestHandler):
    server: SessionPoolServer

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)

        try:
            if parsed.path == "/healthz":
                self._write_json(HTTPStatus.OK, self.server.emulator.health())
                return

            identifier = _identifier(parse_qs(parsed.query))
            if parsed.path == "/files":
                self._write_json(
                    HTTPStatus.OK,
                    self.server.emulator.list_files(identifier=identifier),
                )
                return

            if parsed.path.endswith("/content") and parsed.path.startswith("/files/"):
                filename = unquote(
                    parsed.path[len("/files/") : -len("/content")]
                ).strip("/")
                content_type, content = self.server.emulator.download_file(
                    identifier=identifier,
                    filename=filename,
                )
                self.send_response(HTTPStatus.OK)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(content)))
                self.end_headers()
                self.wfile.write(content)
                return

            self._write_error(HTTPStatus.NOT_FOUND, f"Route {self.path} not found.")
        except FileNotFoundError as error:
            self._write_error(HTTPStatus.NOT_FOUND, str(error))
        except ValueError as error:
            self._write_error(HTTPStatus.BAD_REQUEST, str(error))
        except Exception:  # pragma: no cover - defensive server error path
            LOGGER.exception("Session pool emulator request failed")
            self._write_error(HTTPStatus.INTERNAL_SERVER_ERROR, "Internal Server Error")

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)

        try:
            identifier = _identifier(parse_qs(parsed.query))

            if parsed.path == "/executions":
                body = self._read_json_body()
                code = body.get("code")
                if not isinstance(code, str):
                    raise ValueError("code is required.")
                timeout_seconds = _coerce_timeout(
                    body.get("timeoutInSeconds"),
                    DEFAULT_EXECUTION_TIMEOUT_SECONDS,
                    DEFAULT_EXECUTION_TIMEOUT_SECONDS,
                )
                payload = self.server.emulator.execute(
                    identifier=identifier,
                    code=code,
                    timeout_seconds=timeout_seconds,
                )
                self._write_json(HTTPStatus.OK, payload)
                return

            if parsed.path == "/files":
                filename, content = _multipart_file(
                    self.headers.get("Content-Type", ""),
                    self._read_body(),
                )
                payload = self.server.emulator.upload_file(
                    identifier=identifier,
                    filename=filename,
                    content=content,
                )
                self._write_json(HTTPStatus.OK, payload)
                return

            self._write_error(HTTPStatus.NOT_FOUND, f"Route {self.path} not found.")
        except FileNotFoundError as error:
            self._write_error(HTTPStatus.NOT_FOUND, str(error))
        except ValueError as error:
            self._write_error(HTTPStatus.BAD_REQUEST, str(error))
        except subprocess.TimeoutExpired:
            self._write_error(HTTPStatus.REQUEST_TIMEOUT, "Command timed out.")
        except Exception:  # pragma: no cover - defensive server error path
            LOGGER.exception("Session pool emulator request failed")
            self._write_error(HTTPStatus.INTERNAL_SERVER_ERROR, "Internal Server Error")

    def log_message(self, format: str, *args: object) -> None:  # noqa: A003
        LOGGER.info("%s - %s", self.address_string(), format % args)

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length", "0"))
        return self.rfile.read(length)

    def _read_json_body(self) -> dict[str, Any]:
        return json.loads(self._read_body().decode("utf-8"))

    def _write_json(
        self,
        status: HTTPStatus,
        payload: Any,
        *,
        headers: list[tuple[str, str]] | None = None,
    ) -> None:
        content = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(content)))
        for name, value in headers or []:
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(content)

    def _write_error(self, status: HTTPStatus, message: str) -> None:
        self._write_json(
            status,
            {
                "error": status.phrase,
                "message": message,
                "statusCode": status.value,
            },
        )


class SessionPoolServer(ThreadingHTTPServer):
    def __init__(
        self, server_address: tuple[str, int], emulator: SessionPoolEmulator
    ) -> None:
        super().__init__(server_address, SessionPoolRequestHandler)
        self.emulator = emulator


def create_server(host: str, port: int, workspace_root: Path) -> SessionPoolServer:
    return SessionPoolServer((host, port), SessionPoolEmulator(workspace_root))


def _coerce_timeout(value: Any, default: int, maximum: int) -> int:
    try:
        timeout = int(value)
    except (TypeError, ValueError):
        return default
    return max(1, min(timeout, maximum))


def _identifier(query: dict[str, list[str]]) -> str:
    identifier = query.get("identifier", [""])[0].strip()
    if identifier == "":
        raise ValueError("identifier is required.")
    return identifier


def _multipart_file(content_type: str, body: bytes) -> tuple[str, bytes]:
    if "multipart/form-data" not in content_type:
        raise ValueError("Content-Type must be multipart/form-data.")

    parser = BytesParser(policy=email_policy)
    message = parser.parsebytes(
        f"Content-Type: {content_type}\r\nMIME-Version: 1.0\r\n\r\n".encode("utf-8")
        + body
    )
    if not message.is_multipart():
        raise ValueError("Request body must contain a multipart file.")

    for part in message.iter_parts():
        if part.get_param("name", header="content-disposition") != "file":
            continue
        filename = part.get_filename()
        if not filename:
            raise ValueError("Uploaded file must include a filename.")
        return filename, part.get_payload(decode=True) or b""

    raise ValueError("Multipart body must include a file field.")


def _python_executable(venv_dir: Path) -> Path:
    if os.name == "nt":
        return venv_dir / "Scripts" / "python.exe"
    return venv_dir / "bin" / "python"


def _resolve_mnt_root(workspace_root: Path) -> Path:
    preferred = Path("/mnt")
    try:
        preferred.mkdir(parents=True, exist_ok=True)
        return preferred
    except OSError:
        fallback = workspace_root / ".mnt"
        fallback.mkdir(parents=True, exist_ok=True)
        return fallback


def _point_mnt_data(mnt_dir: Path, target: Path) -> None:
    link_path = mnt_dir / "data"
    temporary_link = mnt_dir / f".ade-data-{uuid.uuid4().hex}"
    if temporary_link.exists():
        temporary_link.unlink()
    temporary_link.symlink_to(target)
    os.replace(temporary_link, link_path)


def _rewrite_mnt_path(mnt_data_path: Path, value: str) -> str:
    return value.replace("/mnt/data", mnt_data_path.as_posix())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="ade-sessionpool-emulator")
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--workspace", type=Path, default=Path("/workspace"))
    parser.add_argument("--log-level", default="INFO")
    args = parser.parse_args(argv)

    logging.basicConfig(
        level=getattr(logging, args.log_level.upper(), logging.INFO),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    server = create_server(args.host, args.port, args.workspace)
    try:
        server.serve_forever()
    except KeyboardInterrupt:  # pragma: no cover - manual shutdown path
        return 0
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":  # pragma: no cover - CLI entrypoint
    raise SystemExit(main())
