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

type PersistedState = {
  configVersionId: string;
  lastSeenSeq: number | null;
  runId: string;
  workspaceId: string;
};

type UploadInstruction = {
  expiresAt: string;
  headers: Record<string, string>;
  method: string;
  url: string;
};

type RunCreatedEvent = {
  runId: string;
  status: string;
};

type RunStatusEvent = {
  operationId?: string | null;
  phase: string;
  runId: string;
  sessionGuid?: string | null;
  state: string;
};

type RunLogEvent = {
  level: string;
  message: string;
  phase: string;
  runId: string;
};

type RunErrorEvent = {
  message: string;
  phase?: string | null;
  retriable: boolean;
  runId: string;
};

type RunResultEvent = {
  outputPath: string;
  runId: string;
  validationIssues: Array<{
    field: string;
    message: string;
    rowIndex: number;
  }>;
};

type RunCompletedEvent = {
  finalStatus: string;
  runId: string;
};

const STORAGE_KEY = "ade.run-poc";

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
  after: number | null,
): string {
  const url = new URL(
    `${buildRunsPath(workspaceId, configVersionId)}/${encodeURIComponent(runId)}/events`,
    window.location.origin,
  );
  if (after !== null) {
    url.searchParams.set("after", String(after));
  }
  return url.toString();
}

function buildCancelPath(
  workspaceId: string,
  configVersionId: string,
  runId: string,
): string {
  return `${buildRunsPath(workspaceId, configVersionId)}/${encodeURIComponent(runId)}/cancel`;
}

function buildUploadsPath(
  workspaceId: string,
  configVersionId: string,
): string {
  return `/api/workspaces/${encodeURIComponent(workspaceId)}/configs/${encodeURIComponent(configVersionId)}/uploads`;
}

function isRunCreatedEvent(value: unknown): value is RunCreatedEvent {
  return typeof value === "object" && value !== null && "runId" in value;
}

function isRunStatusEvent(value: unknown): value is RunStatusEvent {
  return (
    typeof value === "object" &&
    value !== null &&
    "phase" in value &&
    "state" in value
  );
}

function isRunLogEvent(value: unknown): value is RunLogEvent {
  return (
    typeof value === "object" &&
    value !== null &&
    "phase" in value &&
    "level" in value &&
    "message" in value
  );
}

function isRunErrorEvent(value: unknown): value is RunErrorEvent {
  return (
    typeof value === "object" &&
    value !== null &&
    "message" in value &&
    "retriable" in value
  );
}

function isRunResultEvent(value: unknown): value is RunResultEvent {
  return (
    typeof value === "object" &&
    value !== null &&
    "outputPath" in value &&
    "validationIssues" in value
  );
}

function isRunCompletedEvent(value: unknown): value is RunCompletedEvent {
  return typeof value === "object" && value !== null && "finalStatus" in value;
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
      `Request failed with ${String(response.status)}.`
    );
  } catch {
    const text = await response.text();
    return text || `Request failed with ${String(response.status)}.`;
  }
}

export function RunPocPage() {
  const initialState = loadPersistedState();
  const eventSourceRef = useRef<EventSource | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const logViewportRef = useRef<HTMLPreElement | null>(null);
  const statusRef = useRef<RunPocStatus>("idle");
  const [workspaceId, setWorkspaceId] = useState(initialState.workspaceId);
  const [configVersionId, setConfigVersionId] = useState(
    initialState.configVersionId,
  );
  const [runId, setRunId] = useState(initialState.runId);
  const [lastSeenSeq, setLastSeenSeq] = useState(initialState.lastSeenSeq);
  const [outputPath, setOutputPath] = useState<string | null>(null);
  const [status, setStatus] = useState<RunPocStatus>("idle");
  const [statusMessage, setStatusMessage] = useState(
    "Create an upload, send the file directly to storage, then stream run events over SSE.",
  );
  const [logLines, setLogLines] = useState([
    "Temporary ADE run proof of concept.",
    "The browser uploads directly to the returned artifact URL and listens over SSE.",
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
      eventSourceRef.current?.close();
      eventSourceRef.current = null;
    };
  }, []);

  function isActiveEventSource(
    source: EventSource | null,
  ): source is EventSource {
    return source !== null && eventSourceRef.current === source;
  }

  function appendLogLine(line: string) {
    setLogLines((previous) => {
      const next = [...previous, line];
      return next.length > 800 ? next.slice(next.length - 800) : next;
    });
  }

  function closeEventSource() {
    const current = eventSourceRef.current;
    eventSourceRef.current = null;
    current?.close();
  }

  function noteSeq(value: string | null) {
    if (value === null || value.trim() === "") {
      return;
    }

    const parsed = Number.parseInt(value, 10);
    if (!Number.isNaN(parsed)) {
      setLastSeenSeq(parsed);
    }
  }

  function setRunStatus(nextStatus: RunPocStatus, message: string) {
    statusRef.current = nextStatus;
    setStatus(nextStatus);
    setStatusMessage(message);
  }

  function parseEventData<T>(
    data: string,
    guard: (value: unknown) => value is T,
  ): T | null {
    let payload: unknown;
    try {
      payload = JSON.parse(data);
    } catch {
      return null;
    }

    return guard(payload) ? payload : null;
  }

  function addRunEventListener<T>(
    source: EventSource,
    eventName: string,
    guard: (value: unknown) => value is T,
    invalidMessage: string,
    onPayload: (payload: T) => void,
  ) {
    source.addEventListener(eventName, (event) => {
      if (!isActiveEventSource(source)) {
        return;
      }

      const message = event as MessageEvent<string>;
      noteSeq(message.lastEventId);
      const payload = parseEventData(message.data, guard);
      if (!payload) {
        setRunStatus("error", invalidMessage);
        return;
      }

      onPayload(payload);
    });
  }

  function connectToRun(targetRunId: string, after: number | null) {
    closeEventSource();
    setRunStatus("connecting", "Connecting to the run event stream...");

    const source = new EventSource(
      buildEventsPath(workspaceId, configVersionId, targetRunId, after),
    );
    eventSourceRef.current = source;

    source.addEventListener("open", () => {
      if (!isActiveEventSource(source)) {
        return;
      }
      appendLogLine(`[run] connected to ${targetRunId}`);
    });

    addRunEventListener(
      source,
      "run.created",
      isRunCreatedEvent,
      "Received an invalid run.created payload.",
      (payload) => {
        setRunId(payload.runId);
        setRunStatus("streaming", `Run ${payload.runId} created.`);
        appendLogLine(`[run] created ${payload.runId} (${payload.status})`);
      },
    );

    addRunEventListener(
      source,
      "run.status",
      isRunStatusEvent,
      "Received an invalid run.status payload.",
      (payload) => {
        setRunStatus("streaming", `Run phase: ${payload.phase}.`);
        appendLogLine(`[${payload.phase}] ${payload.state}`);
      },
    );

    addRunEventListener(
      source,
      "run.log",
      isRunLogEvent,
      "Received an invalid run.log payload.",
      (payload) => {
        appendLogLine(`[${payload.phase}/${payload.level}] ${payload.message}`);
      },
    );

    addRunEventListener(
      source,
      "run.error",
      isRunErrorEvent,
      "Received an invalid run.error payload.",
      (payload) => {
        setRunStatus("error", payload.message);
        appendLogLine(
          `[error${payload.phase ? `/${payload.phase}` : ""}] ${payload.message}`,
        );
      },
    );

    addRunEventListener(
      source,
      "run.result",
      isRunResultEvent,
      "Received an invalid run.result payload.",
      (payload) => {
        setOutputPath(payload.outputPath);
        appendLogLine(`[result] ${payload.outputPath}`);
      },
    );

    addRunEventListener(
      source,
      "run.completed",
      isRunCompletedEvent,
      "Received an invalid run.completed payload.",
      (payload) => {
        setRunStatus("completed", `Run ${payload.finalStatus}.`);
        appendLogLine(`[complete] ${payload.finalStatus}`);
        source.close();
        if (eventSourceRef.current === source) {
          eventSourceRef.current = null;
        }
      },
    );

    source.onerror = () => {
      if (!isActiveEventSource(source)) {
        return;
      }

      source.close();
      eventSourceRef.current = null;
      if (
        statusRef.current === "connecting" ||
        statusRef.current === "streaming"
      ) {
        setRunStatus("disconnected", "Stream closed. Use Resume to reattach.");
      }
    };
  }

  async function uploadToStorage(file: File, upload: UploadInstruction) {
    const headers = new Headers(upload.headers);
    const response = await fetch(upload.url, {
      method: upload.method,
      headers,
      body: file,
    });

    if (!response.ok) {
      throw new Error(await readErrorMessage(response));
    }
  }

  async function startRun() {
    const file = fileInputRef.current?.files?.[0];
    if (!file) {
      setRunStatus("error", "Choose an input file before starting a run.");
      return;
    }

    closeEventSource();
    setRunId("");
    setLastSeenSeq(null);
    setOutputPath(null);
    setLogLines([
      "Temporary ADE run proof of concept.",
      `Selected input: ${file.name}`,
    ]);

    setRunStatus("uploading", "Requesting upload instructions...");
    const uploadResponse = await fetch(
      buildUploadsPath(workspaceId, configVersionId),
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          filename: file.name,
          contentType: file.type || undefined,
          size: file.size,
        }),
      },
    );

    if (!uploadResponse.ok) {
      setRunStatus("error", await readErrorMessage(uploadResponse));
      return;
    }

    const uploadPayload = (await uploadResponse.json()) as {
      filePath: string;
      upload: UploadInstruction;
      uploadId: string;
    };
    appendLogLine(`[upload] reserved ${uploadPayload.filePath}`);

    try {
      await uploadToStorage(file, uploadPayload.upload);
    } catch (error) {
      setRunStatus(
        "error",
        error instanceof Error ? error.message : "Direct upload failed.",
      );
      return;
    }

    appendLogLine(`[upload] completed ${uploadPayload.uploadId}`);
    setRunStatus("starting", "Creating run...");

    const runResponse = await fetch(
      buildRunsPath(workspaceId, configVersionId),
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          inputPath: uploadPayload.filePath,
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
    connectToRun(payload.runId, null);
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
          Direct upload, async runs, and one-way SSE logs.
        </h2>
        <p className="hero__summary">
          This page validates the new public split: the browser negotiates a
          single-file upload, sends bytes directly to the returned storage URL,
          creates a run over HTTP, and listens on
          <code>{"/runs/{runId}/events"}</code> over server-sent events. The
          terminal stays on its own WebSocket.
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

        <div className="status-grid">
          <section className="status-card">
            <p className="status-card__label">State</p>
            <p className="status-card__value">{status}</p>
          </section>
          <section className="status-card">
            <p className="status-card__label">Run</p>
            <p className="status-card__value status-card__value--mono">
              {runId || "Not started"}
            </p>
          </section>
          <section className="status-card">
            <p className="status-card__label">Last Event</p>
            <p className="status-card__value status-card__value--mono">
              {lastSeenSeq ?? "None"}
            </p>
          </section>
          <section className="status-card">
            <p className="status-card__label">Output</p>
            <p className="status-card__value status-card__value--mono">
              {outputPath ?? "Pending"}
            </p>
          </section>
        </div>

        <p
          className={
            status === "error"
              ? "status-note status-note--error"
              : "status-note"
          }
        >
          {statusMessage}
        </p>

        <p className="status-note">
          Need the bidirectional shell bridge? Open the{" "}
          <Link className="inline-link" to="/terminal-poc">
            terminal POC
          </Link>
          .
        </p>
      </div>

      <pre
        className="terminal-poc__viewport run-poc__viewport"
        ref={logViewportRef}
      >
        {logLines.join("\n")}
      </pre>
    </section>
  );
}
