"""Local Azure-style session pool emulator for ADE development."""

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
import shutil
import subprocess
import sys
import threading
from typing import Any
from urllib.parse import parse_qs, unquote, urlparse
import uuid

LOGGER = logging.getLogger("ade.local.sessionpool")
DEFAULT_COMMAND_TIMEOUT_SECONDS = 120
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


class SessionPoolEmulator:
    def __init__(self, workspace_root: Path) -> None:
        self.workspace_root = workspace_root.resolve()
        self.sessions_root = self.workspace_root / "sessions"
        self.environments_root = self.workspace_root / "environments"
        self.mnt_root = _resolve_mnt_root(self.workspace_root)
        self.mnt_data_path = self.mnt_root / "data"
        self.sessions_root.mkdir(parents=True, exist_ok=True)
        self.environments_root.mkdir(parents=True, exist_ok=True)
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
        target = data_dir / filename
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        return self._file_metadata(target, data_dir)

    def list_files(self, *, identifier: str) -> dict[str, Any]:
        data_dir = self._job_session(identifier).data_dir
        files = [
            {"properties": self._file_metadata(path, data_dir)}
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
    ) -> tuple[dict[str, Any], list[tuple[str, str]]]:
        session = self._job_session(identifier)
        result = self._run_python_code(
            session,
            code,
            timeout_seconds=min(timeout_seconds, DEFAULT_EXECUTION_TIMEOUT_SECONDS),
        )
        operation_id = uuid.uuid4().hex
        headers = [
            ("operation-id", operation_id),
            ("x-ms-session-guid", identifier),
        ]
        return {
            "properties": {
                "stdout": result.stdout,
                "stderr": result.stderr,
                "exitCode": result.exit_code,
                "status": "Succeeded" if result.exit_code == 0 else "Failed",
            }
        }, headers

    def stop_session(self, *, identifier: str) -> dict[str, Any]:
        session_path = self.sessions_root / identifier
        if session_path.exists():
            shutil.rmtree(session_path)
        return {}

    def mcp(self, body: dict[str, Any]) -> dict[str, Any]:
        method = body.get("method")
        request_id = body.get("id")

        if method == "initialize":
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "serverInfo": {"name": "ADE Local Session Pool MCP Server"},
                    "capabilities": {"tools": {"list": True, "call": True}},
                },
            }

        if method == "tools/list":
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "tools": [
                        {"name": "launchShell"},
                        {"name": "runShellCommandInRemoteEnvironment"},
                        {"name": "runPythonCodeInRemoteEnvironment"},
                    ]
                },
            }

        if method != "tools/call":
            return self._mcp_error(request_id, f"Unsupported MCP method: {method}")

        params = body.get("params")
        if not isinstance(params, dict):
            return self._mcp_error(request_id, "MCP params must be an object.")

        tool_name = params.get("name")
        arguments = params.get("arguments")
        if not isinstance(arguments, dict):
            arguments = {}

        if tool_name == "launchShell":
            environment_id = f"env-{uuid.uuid4().hex}"
            self._console_environment(environment_id)
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "structuredContent": {"environmentId": environment_id},
                },
            }

        environment_id = arguments.get("environmentId")
        if not isinstance(environment_id, str) or environment_id.strip() == "":
            return self._mcp_error(request_id, "environmentId is required.")

        try:
            environment = self._console_environment(environment_id)
        except FileNotFoundError:
            return self._mcp_error(
                request_id, f"Environment not found: {environment_id}"
            )

        if tool_name == "runPythonCodeInRemoteEnvironment":
            python_code = arguments.get("pythonCode")
            if not isinstance(python_code, str):
                return self._mcp_error(request_id, "pythonCode is required.")
            timeout_seconds = _coerce_timeout(
                arguments.get("timeoutSeconds"),
                DEFAULT_COMMAND_TIMEOUT_SECONDS,
                900,
            )
            result = self._run_python_code(environment, python_code, timeout_seconds)
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "structuredContent": {
                        "stdout": result.stdout,
                        "stderr": result.stderr,
                        "exitCode": result.exit_code,
                    }
                },
            }

        if tool_name == "runShellCommandInRemoteEnvironment":
            timeout_seconds = _coerce_timeout(
                arguments.get("timeoutSeconds"),
                DEFAULT_COMMAND_TIMEOUT_SECONDS,
                240,
            )
            shell_command = arguments.get("shellCommand")
            exec_command_and_args = arguments.get("execCommandAndArgs")
            try:
                result = self._run_shell_command(
                    environment,
                    shell_command=shell_command
                    if isinstance(shell_command, str)
                    else None,
                    exec_command_and_args=(
                        exec_command_and_args
                        if isinstance(exec_command_and_args, list)
                        else None
                    ),
                    timeout_seconds=timeout_seconds,
                )
            except ValueError as error:
                return self._mcp_error(request_id, str(error))
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "structuredContent": {
                        "stdout": result.stdout,
                        "stderr": result.stderr,
                        "exitCode": result.exit_code,
                    }
                },
            }

        return self._mcp_error(request_id, f"Unsupported MCP tool: {tool_name}")

    def _job_session(self, identifier: str) -> PythonSession:
        return self._ensure_session(self.sessions_root / identifier)

    def _console_environment(self, environment_id: str) -> PythonSession:
        environment_path = self.environments_root / environment_id
        if not environment_path.exists():
            if environment_id.startswith("env-"):
                return self._ensure_session(environment_path)
            raise FileNotFoundError(environment_id)
        return self._ensure_session(environment_path)

    def _ensure_session(self, session_root: Path) -> PythonSession:
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
        return self._run_command(command, session, timeout_seconds=timeout_seconds)

    def _run_shell_command(
        self,
        session: PythonSession,
        *,
        shell_command: str | None,
        exec_command_and_args: list[Any] | None,
        timeout_seconds: int,
    ) -> CommandResult:
        if shell_command:
            command = [
                os.environ.get("SHELL", "/bin/bash"),
                "-lc",
                _rewrite_mnt_path(self.mnt_data_path, shell_command),
            ]
        elif exec_command_and_args:
            command = [
                _rewrite_mnt_path(self.mnt_data_path, str(value))
                for value in exec_command_and_args
            ]
        else:
            raise ValueError("shellCommand or execCommandAndArgs is required.")
        return self._run_command(command, session, timeout_seconds=timeout_seconds)

    def _run_command(
        self,
        command: list[str],
        session: PythonSession,
        *,
        timeout_seconds: int,
    ) -> CommandResult:
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
        return {
            "filename": str(path.relative_to(data_dir)),
            "size": stat.st_size,
            "lastModifiedTime": datetime.fromtimestamp(stat.st_mtime, UTC).isoformat(),
        }

    def _mcp_error(self, request_id: Any, message: str) -> dict[str, Any]:
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32000, "message": message},
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

            if parsed.path.startswith("/files/content/"):
                filename = unquote(parsed.path[len("/files/content/") :]).strip("/")
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
            if parsed.path == "/mcp":
                body = self._read_json_body()
                self._write_json(HTTPStatus.OK, self.server.emulator.mcp(body))
                return

            identifier = _identifier(parse_qs(parsed.query))

            if parsed.path == "/code/execute":
                body = self._read_json_body()
                properties = body.get("properties")
                if not isinstance(properties, dict):
                    raise ValueError("properties is required.")
                code = properties.get("code")
                if not isinstance(code, str):
                    raise ValueError("properties.code is required.")
                timeout_seconds = _coerce_timeout(
                    properties.get("timeoutSeconds"),
                    DEFAULT_EXECUTION_TIMEOUT_SECONDS,
                    DEFAULT_EXECUTION_TIMEOUT_SECONDS,
                )
                payload, headers = self.server.emulator.execute(
                    identifier=identifier,
                    code=code,
                    timeout_seconds=timeout_seconds,
                )
                self._write_json(HTTPStatus.OK, payload, headers=headers)
                return

            if parsed.path == "/files/upload":
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

            if parsed.path == "/.management/stopSession":
                payload = self.server.emulator.stop_session(identifier=identifier)
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
