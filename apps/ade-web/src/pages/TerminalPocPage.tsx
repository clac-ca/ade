import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

type BrowserTerminalEvent =
  | { type: "ready" }
  | { type: "output"; data: string }
  | { type: "error"; message: string }
  | { type: "exit"; code: number | null };

type TerminalStatus =
  | "disconnected"
  | "connecting"
  | "connected"
  | "session-expired"
  | "error";

function buildTerminalWebSocketUrl(
  workspaceId: string,
  configVersionId: string,
): string {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}/api/workspaces/${encodeURIComponent(workspaceId)}/configs/${encodeURIComponent(configVersionId)}/terminal`;
}

function isTerminalEvent(value: unknown): value is BrowserTerminalEvent {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const candidate = value as { type?: unknown };
  return (
    candidate.type === "ready" ||
    candidate.type === "output" ||
    candidate.type === "error" ||
    candidate.type === "exit"
  );
}

export function TerminalPocPage() {
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const socketRef = useRef<WebSocket | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const resizeObserverRef = useRef<ResizeObserver | null>(null);
  const statusRef = useRef<TerminalStatus>("disconnected");
  const [workspaceId, setWorkspaceId] = useState("workspace-a");
  const [configVersionId, setConfigVersionId] = useState("config-v1");
  const [status, setStatus] = useState<TerminalStatus>("disconnected");
  const [statusMessage, setStatusMessage] = useState("Disconnected.");

  function setTerminalStatus(nextStatus: TerminalStatus, message: string) {
    statusRef.current = nextStatus;
    setStatus(nextStatus);
    setStatusMessage(message);
  }

  function writeStatusLine(message: string) {
    terminalRef.current?.writeln(`\r\n[ade] ${message}`);
  }

  function isActiveSocket(socket: WebSocket | null): socket is WebSocket {
    return socket !== null && socketRef.current === socket;
  }

  function sendResize(socket: WebSocket | null = socketRef.current) {
    const terminal = terminalRef.current;
    const fitAddon = fitAddonRef.current;
    if (
      terminal === null ||
      fitAddon === null ||
      socket === null ||
      socket.readyState !== WebSocket.OPEN ||
      statusRef.current !== "connected" ||
      !isActiveSocket(socket)
    ) {
      return;
    }

    fitAddon.fit();
    socket.send(
      JSON.stringify({
        type: "resize",
        rows: terminal.rows,
        cols: terminal.cols,
      }),
    );
  }

  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }

    const terminal = new Terminal({
      convertEol: true,
      cursorBlink: true,
      fontFamily:
        '"SFMono-Regular", "Menlo", "Monaco", "Cascadia Mono", monospace',
      fontSize: 14,
      theme: {
        background: "#0f1720",
        foreground: "#e6edf3",
        cursor: "#7dd3c6",
      },
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(container);
    terminal.writeln("Temporary ADE terminal proof of concept.");
    terminal.writeln("Connect to start a session-backed shell.");
    const fitAnimationFrame = window.requestAnimationFrame(() => {
      fitAddon.fit();
    });

    const inputDisposable = terminal.onData((data) => {
      const socket = socketRef.current;
      if (
        statusRef.current !== "connected" ||
        socket?.readyState !== WebSocket.OPEN
      ) {
        return;
      }

      socket.send(JSON.stringify({ type: "input", data }));
    });

    const handleWindowResize = () => {
      sendResize();
    };

    window.addEventListener("resize", handleWindowResize);
    if (typeof ResizeObserver !== "undefined") {
      const observer = new ResizeObserver(() => {
        sendResize();
      });
      observer.observe(container);
      resizeObserverRef.current = observer;
    }

    terminalRef.current = terminal;
    fitAddonRef.current = fitAddon;

    return () => {
      window.removeEventListener("resize", handleWindowResize);
      window.cancelAnimationFrame(fitAnimationFrame);
      resizeObserverRef.current?.disconnect();
      resizeObserverRef.current = null;
      socketRef.current?.close();
      socketRef.current = null;
      inputDisposable.dispose();
      fitAddonRef.current = null;
      terminalRef.current = null;
      terminal.dispose();
    };
  }, []);

  function closeSocket(socket: WebSocket | null, sendCloseMessage: boolean) {
    if (socket === null) {
      return;
    }

    if (sendCloseMessage && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ type: "close" }));
    }
    socket.close();
  }

  function disconnectTerminal(showMessage = true) {
    const socket = socketRef.current;
    socketRef.current = null;

    if (socket === null) {
      if (showMessage) {
        setTerminalStatus("disconnected", "Disconnected.");
      }
      return;
    }

    closeSocket(socket, true);
    if (showMessage) {
      writeStatusLine("Disconnected.");
      setTerminalStatus("disconnected", "Disconnected.");
    }
  }

  function connectTerminal() {
    disconnectTerminal(false);

    terminalRef.current?.clear();
    terminalRef.current?.writeln("Temporary ADE terminal proof of concept.");
    terminalRef.current?.writeln("Waiting for the session connection...");
    setTerminalStatus("connecting", "Connecting to the terminal...");

    const socket = new WebSocket(
      buildTerminalWebSocketUrl(workspaceId, configVersionId),
    );
    socketRef.current = socket;

    socket.addEventListener("message", (event) => {
      if (!isActiveSocket(socket)) {
        return;
      }

      let payload: unknown;

      try {
        payload = JSON.parse(String(event.data));
      } catch {
        writeStatusLine("Received an invalid terminal message.");
        setTerminalStatus("error", "Received an invalid terminal message.");
        return;
      }

      if (!isTerminalEvent(payload)) {
        writeStatusLine("Received an unknown terminal event.");
        setTerminalStatus("error", "Received an unknown terminal event.");
        return;
      }

      switch (payload.type) {
        case "ready":
          setTerminalStatus("connected", "Connected.");
          sendResize(socket);
          writeStatusLine("Interactive shell connected.");
          return;
        case "output":
          terminalRef.current?.write(payload.data);
          return;
        case "error": {
          const expired = payload.message.includes("expired after 220 seconds");
          writeStatusLine(payload.message);
          setTerminalStatus(
            expired ? "session-expired" : "error",
            expired ? "Session expired." : payload.message,
          );
          return;
        }
        case "exit":
          writeStatusLine(
            payload.code === null
              ? "Shell exited."
              : `Shell exited with code ${String(payload.code)}.`,
          );
          if (statusRef.current === "connected") {
            setTerminalStatus("disconnected", "Disconnected.");
          }
          return;
      }
    });

    socket.addEventListener("close", () => {
      if (!isActiveSocket(socket)) {
        return;
      }

      socketRef.current = null;

      if (
        statusRef.current === "connecting" ||
        statusRef.current === "connected"
      ) {
        setTerminalStatus("disconnected", "Disconnected.");
      }
    });

    socket.addEventListener("error", () => {
      if (!isActiveSocket(socket)) {
        return;
      }

      writeStatusLine("The browser websocket closed unexpectedly.");
      setTerminalStatus("error", "The browser websocket closed unexpectedly.");
    });
  }

  return (
    <section className="panel terminal-poc">
      <div className="hero">
        <p className="eyebrow">Temporary terminal POC</p>
        <h2 className="hero__title">Interactive shell over the session.</h2>
        <p className="hero__summary">
          This page exists only to validate the reverse connection on the
          built-in session pool. Sessions hard-stop after about 220 seconds.
        </p>
      </div>

      <div className="terminal-poc__toolbar">
        <div className="terminal-poc__form">
          <label className="terminal-poc__field">
            <span>Workspace ID</span>
            <input
              className="terminal-poc__input"
              name="workspaceId"
              onChange={(event) => {
                setWorkspaceId(event.target.value);
              }}
              value={workspaceId}
            />
          </label>

          <label className="terminal-poc__field">
            <span>Config Version ID</span>
            <input
              className="terminal-poc__input"
              name="configVersionId"
              onChange={(event) => {
                setConfigVersionId(event.target.value);
              }}
              value={configVersionId}
            />
          </label>

          <div className="terminal-poc__actions">
            <button
              className="terminal-poc__button"
              onClick={connectTerminal}
              type="button"
            >
              Connect
            </button>
            <button
              className="terminal-poc__button terminal-poc__button--secondary"
              onClick={() => {
                disconnectTerminal();
              }}
              type="button"
            >
              Disconnect
            </button>
          </div>
        </div>

        <p className="terminal-poc__status" data-state={status}>
          {statusMessage}
        </p>
      </div>

      <div className="terminal-surface">
        <div className="terminal-surface__viewport" ref={containerRef} />
      </div>

      <p className="status-note">
        This proof page is temporary. Return to the{" "}
        <Link className="inline-link" to="/">
          home page
        </Link>{" "}
        when you are done testing.
      </p>
    </section>
  );
}
