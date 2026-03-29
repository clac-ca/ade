import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";

type RunPocStatus =
  | "idle"
  | "uploading"
  | "starting"
  | "connecting"
  | "streaming"
  | "completed"
  | "disconnected"
  | "error";

type RunSocketEvent =
  | {
      type: "hello";
      lastSeq: number;
      runId: string;
      sessionGuid?: string | null;
      status: string;
    }
  | {
      type: "status";
      seq: number;
      phase: string;
      state: string;
      sessionGuid?: string | null;
      operationId?: string | null;
    }
  | {
      type: "log";
      seq: number;
      level: string;
      message: string;
      phase: string;
    }
  | {
      type: "result";
      seq: number;
      outputPath: string;
      validationIssues: Array<{
        field: string;
        message: string;
        rowIndex: number;
      }>;
    }
  | {
      type: "error";
      seq: number;
      message: string;
      retriable: boolean;
      phase?: string | null;
    }
  | {
      type: "complete";
      seq: number;
      finalStatus: string;
    };

const STORAGE_KEY = "ade.run-poc";

type PersistedState = {
  configVersionId: string;
  lastSeenSeq: number | null;
  runId: string;
  workspaceId: string;
};

function loadPersistedState(): PersistedState {
  if (typeof window === "undefined") {
    return {
      configVersionId: "config-v1",
      lastSeenSeq: null,
      runId: "",
      workspaceId: "workspace-a",
    };
  }

  try {
    const stored = window.sessionStorage.getItem(STORAGE_KEY);
    if (stored === null) {
      throw new Error("missing");
    }
    const parsed = JSON.parse(stored) as Partial<PersistedState>;
    return {
      configVersionId: parsed.configVersionId ?? "config-v1",
      lastSeenSeq:
        typeof parsed.lastSeenSeq === "number" ? parsed.lastSeenSeq : null,
      runId: parsed.runId ?? "",
      workspaceId: parsed.workspaceId ?? "workspace-a",
    };
  } catch {
    return {
      configVersionId: "config-v1",
      lastSeenSeq: null,
      runId: "",
      workspaceId: "workspace-a",
    };
  }
}

function buildRunsPath(workspaceId: string, configVersionId: string): string {
  return `/api/workspaces/${encodeURIComponent(workspaceId)}/configs/${encodeURIComponent(configVersionId)}/runs`;
}

function buildEventsPath(
  workspaceId: string,
  configVersionId: string,
  runId: string,
): string {
  return `${buildRunsPath(workspaceId, configVersionId)}/${encodeURIComponent(runId)}/events`;
}

function buildCancelPath(
  workspaceId: string,
  configVersionId: string,
  runId: string,
): string {
  return `${buildRunsPath(workspaceId, configVersionId)}/${encodeURIComponent(runId)}/cancel`;
}

function buildFilesPath(workspaceId: string, configVersionId: string): string {
  return `/api/workspaces/${encodeURIComponent(workspaceId)}/configs/${encodeURIComponent(configVersionId)}/files`;
}

function buildWebSocketUrl(path: string): string {
  const url = new URL(path, window.location.origin);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

function isRunSocketEvent(value: unknown): value is RunSocketEvent {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const type = (value as { type?: unknown }).type;
  return (
    type === "hello" ||
    type === "status" ||
    type === "log" ||
    type === "result" ||
    type === "error" ||
    type === "complete"
  );
}

async function readErrorMessage(response: Response): Promise<string> {
  try {
    const payload = (await response.json()) as {
      message?: string;
      error?: string;
    };
    return (
      payload.message ??
      payload.error ??
      `Request failed with ${response.status}.`
    );
  } catch {
    const text = await response.text();
    return text || `Request failed with ${response.status}.`;
  }
}

export function RunPocPage() {
  const initialState = loadPersistedState();
  const socketRef = useRef<WebSocket | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const logViewportRef = useRef<HTMLPreElement | null>(null);
  const statusRef = useRef<RunPocStatus>("idle");
  const [workspaceId, setWorkspaceId] = useState(initialState.workspaceId);
  const [configVersionId, setConfigVersionId] = useState(
    initialState.configVersionId,
  );
  const [runId, setRunId] = useState(initialState.runId);
  const [lastSeenSeq, setLastSeenSeq] = useState<number | null>(
    initialState.lastSeenSeq,
  );
  const [outputPath, setOutputPath] = useState<string | null>(null);
  const [status, setStatus] = useState<RunPocStatus>("idle");
  const [statusMessage, setStatusMessage] = useState(
    "Upload a file to start an async run.",
  );
  const [logLines, setLogLines] = useState<string[]>([
    "Temporary ADE run proof of concept.",
    "Uploads are durable. Use Resume to reattach to a live run stream.",
  ]);

  useEffect(() => {
    window.sessionStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        configVersionId,
        lastSeenSeq,
        runId,
        workspaceId,
      } satisfies PersistedState),
    );
  }, [configVersionId, lastSeenSeq, runId, workspaceId]);

  useEffect(() => {
    const viewport = logViewportRef.current;
    if (viewport !== null) {
      viewport.scrollTop = viewport.scrollHeight;
    }
  }, [logLines]);

  useEffect(() => {
    return () => {
      socketRef.current?.close();
      socketRef.current = null;
    };
  }, []);

  function isActiveSocket(socket: WebSocket | null): socket is WebSocket {
    return socket !== null && socketRef.current === socket;
  }

  function setRunStatus(nextStatus: RunPocStatus, message: string) {
    statusRef.current = nextStatus;
    setStatus(nextStatus);
    setStatusMessage(message);
  }

  function appendLogLine(line: string) {
    setLogLines((previous) => {
      const next = [...previous, line];
      return next.length > 800 ? next.slice(next.length - 800) : next;
    });
  }

  function closeSocket() {
    const socket = socketRef.current;
    socketRef.current = null;
    socket?.close();
  }

  function noteSeq(seq: number) {
    setLastSeenSeq(seq);
  }

  function handleRunEvent(event: RunSocketEvent) {
    switch (event.type) {
      case "hello":
        setRunId(event.runId);
        setRunStatus(
          event.status === "succeeded" ||
            event.status === "failed" ||
            event.status === "cancelled"
            ? "completed"
            : "streaming",
          `Attached to run ${event.runId}.`,
        );
        appendLogLine(
          `[run] attached to ${event.runId} (status=${event.status}, lastSeq=${String(event.lastSeq)})`,
        );
        return;
      case "status":
        noteSeq(event.seq);
        setRunStatus("streaming", `Run phase: ${event.phase}.`);
        appendLogLine(`[${event.phase}] ${event.state}`);
        return;
      case "log":
        noteSeq(event.seq);
        appendLogLine(`[${event.phase}/${event.level}] ${event.message}`);
        return;
      case "result":
        noteSeq(event.seq);
        setOutputPath(event.outputPath);
        appendLogLine(`[result] ${event.outputPath}`);
        return;
      case "error":
        noteSeq(event.seq);
        setRunStatus("error", event.message);
        appendLogLine(
          `[error${event.phase ? `/${event.phase}` : ""}] ${event.message}`,
        );
        return;
      case "complete":
        noteSeq(event.seq);
        setRunStatus("completed", `Run ${event.finalStatus}.`);
        appendLogLine(`[complete] ${event.finalStatus}`);
    }
  }

  function connectToRun(
    targetRunId: string,
    attachSeq: number | null,
    eventsPath = buildEventsPath(workspaceId, configVersionId, targetRunId),
  ) {
    closeSocket();
    setRunStatus("connecting", "Connecting to the run events stream...");

    const socket = new WebSocket(buildWebSocketUrl(eventsPath));
    socketRef.current = socket;

    socket.addEventListener("open", () => {
      if (!isActiveSocket(socket)) {
        return;
      }

      socket.send(
        JSON.stringify({
          type: "attach",
          lastSeenSeq: attachSeq,
        }),
      );
    });

    socket.addEventListener("message", (event) => {
      if (!isActiveSocket(socket)) {
        return;
      }

      let payload: unknown;
      try {
        payload = JSON.parse(String(event.data));
      } catch {
        appendLogLine("[error] received an invalid websocket payload");
        setRunStatus("error", "Received an invalid websocket payload.");
        return;
      }

      if (!isRunSocketEvent(payload)) {
        appendLogLine("[error] received an unknown websocket event");
        setRunStatus("error", "Received an unknown websocket event.");
        return;
      }

      handleRunEvent(payload);
    });

    socket.addEventListener("close", () => {
      if (!isActiveSocket(socket)) {
        return;
      }

      socketRef.current = null;
      if (
        statusRef.current === "connecting" ||
        statusRef.current === "streaming"
      ) {
        setRunStatus("disconnected", "Stream closed. Use Resume to reattach.");
      }
    });

    socket.addEventListener("error", () => {
      if (!isActiveSocket(socket)) {
        return;
      }

      appendLogLine("[error] websocket closed unexpectedly");
      setRunStatus("error", "The run events websocket closed unexpectedly.");
    });
  }

  async function startRun() {
    const file = fileInputRef.current?.files?.[0];
    if (!file) {
      setRunStatus("error", "Choose an input file before starting a run.");
      return;
    }

    closeSocket();
    setRunId("");
    setLastSeenSeq(null);
    setOutputPath(null);
    setLogLines([
      "Temporary ADE run proof of concept.",
      `Selected input: ${file.name}`,
    ]);

    setRunStatus("uploading", "Uploading input file...");
    const uploadBody = new FormData();
    uploadBody.set("file", file);

    const uploadResponse = await fetch(
      buildFilesPath(workspaceId, configVersionId),
      {
        method: "POST",
        body: uploadBody,
      },
    );
    if (!uploadResponse.ok) {
      setRunStatus("error", await readErrorMessage(uploadResponse));
      return;
    }

    const uploaded = (await uploadResponse.json()) as { filename: string };
    appendLogLine(`[upload] stored ${uploaded.filename}`);
    setRunStatus("starting", "Starting async run...");

    const runResponse = await fetch(
      buildRunsPath(workspaceId, configVersionId),
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Prefer: "respond-async",
        },
        body: JSON.stringify({
          inputPath: uploaded.filename,
        }),
      },
    );
    if (!runResponse.ok) {
      setRunStatus("error", await readErrorMessage(runResponse));
      return;
    }

    const payload = (await runResponse.json()) as {
      eventsUrl: string;
      runId: string;
      status: string;
    };

    setRunId(payload.runId);
    appendLogLine(`[run] accepted ${payload.runId} (${payload.status})`);
    connectToRun(payload.runId, null, payload.eventsUrl);
  }

  async function cancelRun() {
    if (!runId) {
      setRunStatus("error", "Enter a run ID before cancelling.");
      return;
    }

    const response = await fetch(
      buildCancelPath(workspaceId, configVersionId, runId),
      {
        method: "POST",
      },
    );
    if (!response.ok) {
      setRunStatus("error", await readErrorMessage(response));
      return;
    }

    appendLogLine(`[run] cancel requested for ${runId}`);
  }

  return (
    <section className="panel run-poc">
      <div className="hero">
        <p className="eyebrow">Temporary async run POC</p>
        <h2 className="hero__title">
          Stream config install and run output over `/runs`.
        </h2>
        <p className="hero__summary">
          This page validates the unified async run flow: durable file upload,
          `Prefer: respond-async`, and resumable event replay on
          <code>{"/runs/{runId}/events"}</code>. Runs still inherit the built-in
          session execution cap of about 220 seconds.
        </p>
      </div>

      <div className="run-poc__toolbar">
        <div className="run-poc__form">
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

          <label className="terminal-poc__field">
            <span>Run ID</span>
            <input
              className="terminal-poc__input"
              name="runId"
              onChange={(event) => {
                setRunId(event.target.value);
              }}
              value={runId}
            />
          </label>

          <label className="terminal-poc__field">
            <span>Input File</span>
            <input
              className="terminal-poc__input"
              name="inputFile"
              ref={fileInputRef}
              type="file"
            />
          </label>

          <div className="terminal-poc__actions">
            <button
              className="terminal-poc__button"
              onClick={() => {
                void startRun();
              }}
              type="button"
            >
              Start Run
            </button>
            <button
              className="terminal-poc__button terminal-poc__button--secondary"
              onClick={() => {
                if (!runId) {
                  setRunStatus("error", "Enter a run ID before resuming.");
                  return;
                }
                connectToRun(runId, lastSeenSeq);
              }}
              type="button"
            >
              Resume
            </button>
            <button
              className="terminal-poc__button terminal-poc__button--secondary"
              onClick={() => {
                void cancelRun();
              }}
              type="button"
            >
              Cancel
            </button>
          </div>
        </div>

        <p className="terminal-poc__status" data-state={status}>
          {statusMessage}
        </p>
      </div>

      <div className="status-grid">
        <section className="status-card">
          <p className="status-card__label">Run ID</p>
          <p className="status-card__value status-card__value--mono">
            {runId || "Not started"}
          </p>
        </section>
        <section className="status-card">
          <p className="status-card__label">Last Seq</p>
          <p className="status-card__value status-card__value--mono">
            {lastSeenSeq === null ? "None" : String(lastSeenSeq)}
          </p>
        </section>
        <section className="status-card">
          <p className="status-card__label">Output Path</p>
          <p className="status-card__value status-card__value--mono">
            {outputPath ?? "Pending"}
          </p>
        </section>
      </div>

      <div className="run-log">
        <pre className="run-log__viewport" ref={logViewportRef}>
          {logLines.join("\n")}
        </pre>
      </div>

      <p className="status-note">
        This proof page is temporary. Keep the{" "}
        <Link className="inline-link" to="/terminal-poc">
          terminal POC
        </Link>{" "}
        for raw shell debugging, and return to the{" "}
        <Link className="inline-link" to="/">
          home page
        </Link>{" "}
        when you are done testing.
      </p>
    </section>
  );
}
