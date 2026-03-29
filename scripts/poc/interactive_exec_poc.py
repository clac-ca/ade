#!/usr/bin/env python3
"""Proof-of-concept interactive exec bridge for Azure-style PythonLTS sessions.

This script exercises the most viable built-in-session design:

1. Start a local WebSocket relay on the host.
2. Execute inline Python in the session pool.
3. The inline Python opens a reverse WebSocket back to the relay.
4. The relay drives a PTY-backed shell inside the session.

The same harness supports the local session-pool emulator and the real Azure
session-pool endpoint. The Azure path still requires:

- a relay URL reachable from Azure egress
- Azure ContainerApps Session Executor permissions on the target pool
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import json
import secrets
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from typing import Any

import requests
from websockets.asyncio.server import ServerConnection, serve

DEFAULT_API_VERSION = "2025-10-02-preview"
DEFAULT_LOCAL_ENDPOINT = "http://127.0.0.1:8014"
DEFAULT_LOCAL_RELAY_BIND = "127.0.0.1"
DEFAULT_LOCAL_RELAY_CONNECT = "host.docker.internal"
DEFAULT_RELAY_PORT = 8765
DEFAULT_SESSION_TIMEOUT_SECONDS = 180

BOOTSTRAP_TEMPLATE = r"""
import asyncio
import base64
import errno
import json
import os
import pty
import shutil
import signal
import subprocess
import sys

CONFIG = json.loads("__CONFIG_JSON__")


def _install_websockets():
    try:
        import websockets  # noqa: F401
    except Exception:
        subprocess.run(
            [
                sys.executable,
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--quiet",
                "websockets>=15,<16",
            ],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    import websockets
    return websockets


def _set_winsize(fd: int, rows: int, cols: int) -> None:
    try:
        import fcntl
        import struct
        import termios

        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    except Exception:
        return


def _decode(message: str) -> dict[str, object]:
    return json.loads(message)


def _encode(message: dict[str, object]) -> str:
    return json.dumps(message, separators=(",", ":"))


async def main() -> None:
    websockets = _install_websockets()

    shell = shutil.which(CONFIG["shell"]) or shutil.which("bash") or shutil.which("sh")
    if shell is None:
        raise RuntimeError("Could not locate a shell executable inside the session.")

    master_fd, slave_fd = pty.openpty()
    _set_winsize(slave_fd, CONFIG["rows"], CONFIG["cols"])
    process = subprocess.Popen(
        [shell, "-i"],
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        cwd=CONFIG["cwd"],
        start_new_session=True,
        close_fds=True,
    )
    os.close(slave_fd)
    os.set_blocking(master_fd, False)

    async with websockets.connect(
        CONFIG["relayUrl"],
        additional_headers=[("x-poc-token", CONFIG["token"])],
        ping_interval=20,
        ping_timeout=20,
        max_size=None,
    ) as websocket:
        await websocket.send(
            _encode(
                {
                    "type": "ready",
                    "pid": process.pid,
                    "shell": shell,
                }
            )
        )

        async def forward_pty() -> None:
            while True:
                try:
                    data = os.read(master_fd, 4096)
                except BlockingIOError:
                    if process.poll() is not None:
                        break
                    await asyncio.sleep(0.01)
                    continue
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise

                if not data:
                    if process.poll() is not None:
                        break
                    await asyncio.sleep(0.01)
                    continue

                await websocket.send(
                    _encode(
                        {
                            "type": "stdout",
                            "data": base64.b64encode(data).decode("ascii"),
                        }
                    )
                )

            return_code = process.wait()
            await websocket.send(
                _encode(
                    {
                        "type": "exit",
                        "returnCode": return_code,
                    }
                )
            )

        async def forward_control() -> None:
            async for raw_message in websocket:
                message = _decode(raw_message)
                message_type = message.get("type")

                if message_type == "stdin":
                    os.write(master_fd, base64.b64decode(message["data"]))
                    continue

                if message_type == "resize":
                    _set_winsize(
                        master_fd,
                        int(message.get("rows", CONFIG["rows"])),
                        int(message.get("cols", CONFIG["cols"])),
                    )
                    continue

                if message_type == "signal":
                    signal_name = str(message.get("name", "SIGTERM"))
                    signal_value = getattr(signal, signal_name, signal.SIGTERM)
                    try:
                        os.killpg(os.getpgid(process.pid), signal_value)
                    except ProcessLookupError:
                        pass
                    continue

                if message_type == "close":
                    try:
                        os.killpg(os.getpgid(process.pid), signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    break

        try:
            await asyncio.gather(forward_pty(), forward_control())
        finally:
            try:
                os.close(master_fd)
            except OSError:
                pass
            if process.poll() is None:
                try:
                    os.killpg(os.getpgid(process.pid), signal.SIGTERM)
                except ProcessLookupError:
                    pass
            print("__POC_BRIDGE_DONE__")


asyncio.run(main())
"""


@dataclass
class RelayState:
    token: str
    websocket: ServerConnection | None = None
    transcript: list[str] = field(default_factory=list)
    ready: asyncio.Event = field(default_factory=asyncio.Event)
    exited: asyncio.Event = field(default_factory=asyncio.Event)
    exit_code: int | None = None

    def append_output(self, chunk: str) -> None:
        self.transcript.append(chunk)

    def output_text(self) -> str:
        return "".join(self.transcript)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="interactive_exec_poc")
    parser.add_argument(
        "--mode",
        choices=("local", "azure"),
        default="local",
        help="local uses the Dockerized ADE session-pool emulator; azure targets a live Azure pool.",
    )
    parser.add_argument(
        "--session-endpoint",
        default=DEFAULT_LOCAL_ENDPOINT,
        help="Session-pool management endpoint base URL.",
    )
    parser.add_argument(
        "--session-identifier",
        default=f"codex-poc-{int(time.time())}",
        help="Stable identifier for the session.",
    )
    parser.add_argument(
        "--timeout-seconds",
        type=int,
        default=DEFAULT_SESSION_TIMEOUT_SECONDS,
        help="Maximum session execution lifetime.",
    )
    parser.add_argument(
        "--relay-bind-host",
        default=DEFAULT_LOCAL_RELAY_BIND,
        help="Host interface the local relay server binds to.",
    )
    parser.add_argument(
        "--relay-connect-host",
        default=DEFAULT_LOCAL_RELAY_CONNECT,
        help="Host name the in-session process should use when dialing back to the relay.",
    )
    parser.add_argument(
        "--relay-port",
        type=int,
        default=DEFAULT_RELAY_PORT,
        help="TCP port for the reverse WebSocket relay.",
    )
    parser.add_argument(
        "--relay-url",
        default=None,
        help="Explicit relay WebSocket URL, used mainly for Azure validation with a public tunnel.",
    )
    parser.add_argument(
        "--api-version",
        default=DEFAULT_API_VERSION,
        help="Session-pool data-plane API version.",
    )
    parser.add_argument(
        "--shell",
        default="bash",
        help="Preferred shell inside the session.",
    )
    parser.add_argument(
        "--cwd",
        default="/mnt/data",
        help="Working directory inside the session.",
    )
    parser.add_argument(
        "--rows",
        type=int,
        default=32,
        help="Initial terminal rows.",
    )
    parser.add_argument(
        "--cols",
        type=int,
        default=120,
        help="Initial terminal columns.",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Print PTY output live as the session writes it.",
    )
    return parser.parse_args()


def build_bootstrap_code(
    *,
    relay_url: str,
    token: str,
    shell: str,
    cwd: str,
    rows: int,
    cols: int,
) -> str:
    config = {
        "relayUrl": relay_url,
        "token": token,
        "shell": shell,
        "cwd": cwd,
        "rows": rows,
        "cols": cols,
    }
    return BOOTSTRAP_TEMPLATE.replace(
        '"__CONFIG_JSON__"',
        json.dumps(json.dumps(config)),
    )


def execute_code(
    *,
    mode: str,
    endpoint: str,
    identifier: str,
    code: str,
    timeout_seconds: int,
    api_version: str,
) -> dict[str, Any]:
    url = (
        f"{endpoint.rstrip('/')}/executions"
        f"?identifier={identifier}&api-version={api_version}"
    )
    payload = {
        "code": code,
        "codeInputType": "Inline",
        "executionType": "Synchronous",
        "timeoutInSeconds": timeout_seconds,
    }
    headers = {"Content-Type": "application/json"}

    if mode == "azure":
        token = subprocess.run(
            [
                "az",
                "account",
                "get-access-token",
                "--resource",
                "https://dynamicsessions.io",
                "--query",
                "accessToken",
                "-o",
                "tsv",
            ],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        headers["Authorization"] = f"Bearer {token}"

    response = requests.post(url, headers=headers, json=payload, timeout=timeout_seconds + 30)
    response.raise_for_status()
    return response.json()


def run_execution_in_thread(
    *,
    mode: str,
    endpoint: str,
    identifier: str,
    code: str,
    timeout_seconds: int,
    api_version: str,
) -> tuple[threading.Thread, dict[str, Any]]:
    result: dict[str, Any] = {}

    def runner() -> None:
        try:
            result["response"] = execute_code(
                mode=mode,
                endpoint=endpoint,
                identifier=identifier,
                code=code,
                timeout_seconds=timeout_seconds,
                api_version=api_version,
            )
        except Exception as error:  # pragma: no cover - surfaced by caller
            result["error"] = error

    thread = threading.Thread(target=runner, daemon=True)
    thread.start()
    return thread, result


async def relay_handler(
    websocket: ServerConnection,
    state: RelayState,
    verbose: bool,
) -> None:
    if websocket.request.headers.get("x-poc-token") != state.token:
        await websocket.close(code=4401, reason="invalid token")
        return

    state.websocket = websocket

    try:
        async for raw_message in websocket:
            message = json.loads(raw_message)
            message_type = message.get("type")

            if message_type == "ready":
                state.ready.set()
                continue

            if message_type == "stdout":
                chunk = base64.b64decode(message["data"]).decode("utf-8", "replace")
                state.append_output(chunk)
                if verbose:
                    print(chunk, end="", flush=True)
                continue

            if message_type == "exit":
                state.exit_code = int(message.get("returnCode", 0))
                state.exited.set()
                continue
    finally:
        state.exited.set()


async def send_stdin(state: RelayState, command: str) -> None:
    if state.websocket is None:
        raise RuntimeError("Relay client is not connected.")
    await state.websocket.send(
        json.dumps(
            {
                "type": "stdin",
                "data": base64.b64encode(command.encode("utf-8")).decode("ascii"),
            },
            separators=(",", ":"),
        )
    )


async def send_resize(state: RelayState, rows: int, cols: int) -> None:
    if state.websocket is None:
        raise RuntimeError("Relay client is not connected.")
    await state.websocket.send(
        json.dumps(
            {"type": "resize", "rows": rows, "cols": cols},
            separators=(",", ":"),
        )
    )


async def close_session(state: RelayState) -> None:
    if state.websocket is None:
        return
    try:
        await state.websocket.send(json.dumps({"type": "close"}, separators=(",", ":")))
    except Exception:
        return


async def wait_for_output(state: RelayState, marker: str, timeout_seconds: float) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if marker in state.output_text():
            return
        await asyncio.sleep(0.05)
    raise TimeoutError(f"Timed out waiting for marker: {marker}")


async def exercise_bridge(state: RelayState) -> None:
    await send_resize(state, 40, 120)

    steps = [
        ("pwd\nprintf '__STEP1__\\n'\n", "__STEP1__"),
        ("python -c 'print(\"py-ok\")'\nprintf '__STEP2__\\n'\n", "__STEP2__"),
        ("stty size\nprintf '__STEP3__\\n'\n", "__STEP3__"),
    ]

    for command, marker in steps:
        await send_stdin(state, command)
        await wait_for_output(state, marker, timeout_seconds=20)

    await send_stdin(state, "exit\n")
    await asyncio.wait_for(state.exited.wait(), timeout=20)


async def wait_for_ready_or_exec_failure(
    state: RelayState,
    execution_thread: threading.Thread,
    execution_result: dict[str, Any],
    timeout_seconds: float,
) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if state.ready.is_set():
            return
        if not execution_thread.is_alive():
            if "error" in execution_result:
                raise execution_result["error"]
            raise RuntimeError("Session execution finished before the relay connected.")
        await asyncio.sleep(0.05)
    raise TimeoutError("Timed out waiting for the reverse relay connection.")


def default_relay_url(args: argparse.Namespace, channel_id: str) -> str:
    if args.relay_url is not None:
        return args.relay_url
    return f"ws://{args.relay_connect_host}:{args.relay_port}/bridge/{channel_id}"


async def async_main(args: argparse.Namespace) -> int:
    channel_id = secrets.token_urlsafe(12)
    token = secrets.token_urlsafe(24)
    relay_url = default_relay_url(args, channel_id)
    state = RelayState(token=token)
    bootstrap_code = build_bootstrap_code(
        relay_url=relay_url,
        token=token,
        shell=args.shell,
        cwd=args.cwd,
        rows=args.rows,
        cols=args.cols,
    )

    endpoint = args.session_endpoint
    execution_thread, execution_result = run_execution_in_thread(
        mode=args.mode,
        endpoint=endpoint,
        identifier=args.session_identifier,
        code=bootstrap_code,
        timeout_seconds=args.timeout_seconds,
        api_version=args.api_version,
    )

    async with serve(
        lambda ws: relay_handler(ws, state, args.verbose),
        args.relay_bind_host,
        args.relay_port,
        max_size=None,
    ):
        try:
            await wait_for_ready_or_exec_failure(
                state,
                execution_thread,
                execution_result,
                timeout_seconds=60,
            )
            await exercise_bridge(state)
        finally:
            await close_session(state)

    execution_thread.join(timeout=args.timeout_seconds + 30)
    if execution_thread.is_alive():
        raise TimeoutError("Session-pool execution thread did not complete in time.")
    if "error" in execution_result:
        raise execution_result["error"]

    response = execution_result.get("response", {})
    print("\n=== POC Summary ===")
    print(f"mode: {args.mode}")
    print(f"session_identifier: {args.session_identifier}")
    print(f"relay_url: {relay_url}")
    print(f"shell_exit_code: {state.exit_code}")
    print("execution_response:")
    print(json.dumps(response, indent=2))
    print("transcript:")
    print(state.output_text())
    return 0


def main() -> int:
    args = parse_args()
    try:
        return asyncio.run(async_main(args))
    except requests.HTTPError as error:
        response = error.response
        if response is not None:
            print(f"HTTP {response.status_code} from session pool.", file=sys.stderr)
            if response.text.strip():
                print(response.text, file=sys.stderr)
        else:
            print(str(error), file=sys.stderr)
        return 1
    except Exception as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
