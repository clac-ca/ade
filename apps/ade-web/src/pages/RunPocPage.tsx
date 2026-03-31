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

type RunScope = {
  configVersionId: string;
  workspaceId: string;
};

type ArtifactAccessInstruction = {
  expiresAt: string;
  headers: Record<string, string>;
  method: string;
  url: string;
};

type RunDetail = {
  errorMessage?: string | null;
  logPath?: string | null;
  outputPath?: string | null;
  status: string;
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
  logPath?: string | null;
  outputPath?: string | null;
  runId: string;
};

type BulkItemStatus =
  | "selected"
  | "uploading"
  | "uploaded"
  | "runPending"
  | "running"
  | "succeeded"
  | "failed";

type BulkItem = {
  errorMessage: string | null;
  fileId: string;
  fileName: string;
  filePath: string;
  logPath: string | null;
  outputPath: string | null;
  runId: string | null;
  size: number;
  status: BulkItemStatus;
  uploadedBytes: number;
};

type PersistedBulkSession = {
  batchId: string;
  items: BulkItem[];
  scope: RunScope;
};

type PersistedState = {
  activeRunScope: RunScope | null;
  bulkSession: PersistedBulkSession | null;
  configVersionId: string;
  lastSeenSeq: number | null;
  runId: string;
  workspaceId: string;
};

type BulkUploadBatchResponse = {
  batchId: string;
  items: Array<{
    fileId: string;
    filePath: string;
    upload: ArtifactAccessInstruction;
  }>;
};

const STORAGE_KEY = "ade.run-poc";
const BULK_UPLOAD_FILE_CONCURRENCY = 4;
const BULK_POLL_INTERVAL_MS = 5_000;
const DEFAULT_SCOPE: RunScope = {
  configVersionId: "config-v1",
  workspaceId: "workspace-a",
};
const DEFAULT_PERSISTED_STATE: PersistedState = {
  activeRunScope: null,
  bulkSession: null,
  configVersionId: DEFAULT_SCOPE.configVersionId,
  lastSeenSeq: null,
  runId: "",
  workspaceId: DEFAULT_SCOPE.workspaceId,
};
function isBulkTerminalStatus(status: BulkItemStatus): boolean {
  return status === "succeeded" || status === "failed";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseRunScope(value: unknown): RunScope | null {
  if (!isRecord(value)) {
    return null;
  }
  const scope = value as Partial<RunScope>;
  if (
    typeof scope.workspaceId !== "string" ||
    scope.workspaceId.trim() === "" ||
    typeof scope.configVersionId !== "string" ||
    scope.configVersionId.trim() === ""
  ) {
    return null;
  }
  return {
    configVersionId: scope.configVersionId,
    workspaceId: scope.workspaceId,
  };
}

function parseBulkStatus(value: unknown): BulkItemStatus {
  switch (value) {
    case "selected":
    case "uploading":
    case "uploaded":
    case "runPending":
    case "running":
    case "succeeded":
    case "failed":
      return value;
    default:
      return "failed";
  }
}

function restoreBulkSession(value: unknown): PersistedBulkSession | null {
  if (!isRecord(value)) {
    return null;
  }
  const session = value as Partial<PersistedBulkSession>;

  const batchId =
    typeof session.batchId === "string" && session.batchId.trim() !== ""
      ? session.batchId
      : null;
  const scope = parseRunScope(session.scope);
  if (!batchId || !scope || !Array.isArray(session.items)) {
    return null;
  }

  const items = session.items
    .map((item): BulkItem | null => {
      if (!isRecord(item)) {
        return null;
      }
      const candidate = item as Partial<BulkItem>;
      if (
        typeof candidate.fileId !== "string" ||
        candidate.fileId.trim() === "" ||
        typeof candidate.fileName !== "string" ||
        candidate.fileName.trim() === "" ||
        typeof candidate.filePath !== "string" ||
        candidate.filePath.trim() === "" ||
        typeof candidate.size !== "number" ||
        !Number.isFinite(candidate.size) ||
        candidate.size <= 0
      ) {
        return null;
      }

      const runId =
        typeof candidate.runId === "string" && candidate.runId.trim() !== ""
          ? candidate.runId
          : null;
      let status = parseBulkStatus(candidate.status);
      let errorMessage =
        typeof candidate.errorMessage === "string" &&
        candidate.errorMessage.trim() !== ""
          ? candidate.errorMessage
          : null;
      if (!runId && !isBulkTerminalStatus(status)) {
        status = "failed";
        errorMessage ??= "Page reload interrupted the upload. Retry this file.";
      }

      const uploadedBytes = Math.max(
        0,
        Math.min(
          typeof candidate.uploadedBytes === "number" &&
            Number.isFinite(candidate.uploadedBytes)
            ? candidate.uploadedBytes
            : 0,
          candidate.size,
        ),
      );

      return {
        errorMessage,
        fileId: candidate.fileId,
        fileName: candidate.fileName,
        filePath: candidate.filePath,
        logPath:
          typeof candidate.logPath === "string" &&
          candidate.logPath.trim() !== ""
            ? candidate.logPath
            : null,
        outputPath:
          typeof candidate.outputPath === "string" &&
          candidate.outputPath.trim() !== ""
            ? candidate.outputPath
            : null,
        runId,
        size: candidate.size,
        status,
        uploadedBytes:
          status === "uploaded" ||
          status === "runPending" ||
          status === "running"
            ? candidate.size
            : uploadedBytes,
      };
    })
    .filter((item): item is BulkItem => item !== null);

  return {
    batchId,
    items,
    scope,
  };
}

function loadPersistedState(): PersistedState {
  if (typeof window === "undefined") {
    return DEFAULT_PERSISTED_STATE;
  }

  try {
    const stored = window.sessionStorage.getItem(STORAGE_KEY);
    if (stored === null) {
      return DEFAULT_PERSISTED_STATE;
    }

    const parsed = JSON.parse(stored) as Partial<PersistedState>;
    return {
      activeRunScope: parseRunScope(parsed.activeRunScope),
      bulkSession: restoreBulkSession(parsed.bulkSession),
      configVersionId:
        typeof parsed.configVersionId === "string" &&
        parsed.configVersionId.trim() !== ""
          ? parsed.configVersionId
          : DEFAULT_SCOPE.configVersionId,
      lastSeenSeq:
        typeof parsed.lastSeenSeq === "number" ? parsed.lastSeenSeq : null,
      runId: typeof parsed.runId === "string" ? parsed.runId : "",
      workspaceId:
        typeof parsed.workspaceId === "string" &&
        parsed.workspaceId.trim() !== ""
          ? parsed.workspaceId
          : DEFAULT_SCOPE.workspaceId,
    };
  } catch {
    return DEFAULT_PERSISTED_STATE;
  }
}

function buildRunsPath(workspaceId: string, configVersionId: string): string {
  return `/api/workspaces/${encodeURIComponent(workspaceId)}/configs/${encodeURIComponent(configVersionId)}/runs`;
}

function buildRunPath(scope: RunScope, runId: string): string {
  return `${buildRunsPath(scope.workspaceId, scope.configVersionId)}/${encodeURIComponent(runId)}`;
}

function buildEventsPath(
  scope: RunScope,
  runId: string,
  after: number | null,
): string {
  const url = new URL(
    `${buildRunPath(scope, runId)}/events`,
    window.location.origin,
  );
  if (after !== null) {
    url.searchParams.set("after", String(after));
  }
  return url.toString();
}

function buildCancelPath(scope: RunScope, runId: string): string {
  return `${buildRunPath(scope, runId)}/cancel`;
}

function buildUploadsPath(
  workspaceId: string,
  configVersionId: string,
): string {
  return `/api/workspaces/${encodeURIComponent(workspaceId)}/configs/${encodeURIComponent(configVersionId)}/uploads`;
}

function buildUploadBatchesPath(
  workspaceId: string,
  configVersionId: string,
): string {
  return `${buildUploadsPath(workspaceId, configVersionId)}/batches`;
}

function buildDownloadsPath(scope: RunScope, runId: string): string {
  return `${buildRunPath(scope, runId)}/downloads`;
}

function mapRunStatusToBulkStatus(status: string): BulkItemStatus {
  switch (status) {
    case "pending":
      return "runPending";
    case "running":
      return "running";
    case "succeeded":
      return "succeeded";
    default:
      return "failed";
  }
}

function shouldPollBulkItem(item: BulkItem): boolean {
  return item.runId !== null && !isBulkTerminalStatus(item.status);
}

function bulkProgressLabel(item: BulkItem): string {
  if (item.status === "failed") {
    return "failed";
  }
  if (item.size <= 0) {
    return "0%";
  }

  const completedBytes =
    item.status === "uploaded" ||
    item.status === "runPending" ||
    item.status === "running" ||
    item.status === "succeeded"
      ? item.size
      : item.uploadedBytes;
  const percent = Math.round((completedBytes / item.size) * 100);
  return `${Math.max(0, Math.min(percent, 100))}%`;
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

async function uploadToStorage(
  file: File,
  upload: ArtifactAccessInstruction,
  onProgress: (loadedBytes: number) => void,
) {
  const headers = new Headers();
  for (const [name, value] of Object.entries(upload.headers)) {
    headers.set(name, value);
  }
  if (!headers.has("content-type") && file.type) {
    headers.set("content-type", file.type);
  }

  const response = await fetch(upload.url, {
    body: file,
    headers,
    method: upload.method,
  });
  if (!response.ok) {
    throw new Error(await readErrorMessage(response));
  }

  onProgress(file.size);
}

export function RunPocPage() {
  const [persistedState] = useState(loadPersistedState);
  const eventSourceRef = useRef<EventSource | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const logViewportRef = useRef<HTMLPreElement | null>(null);
  const statusRef = useRef<RunPocStatus>("idle");
  const bulkSessionRef = useRef<PersistedBulkSession | null>(
    persistedState.bulkSession,
  );
  const bulkPollInFlightRef = useRef(false);
  const [workspaceId, setWorkspaceId] = useState(persistedState.workspaceId);
  const [configVersionId, setConfigVersionId] = useState(
    persistedState.configVersionId,
  );
  const [runId, setRunId] = useState(persistedState.runId);
  const [activeRunScope, setActiveRunScope] = useState<RunScope | null>(
    persistedState.activeRunScope,
  );
  const [lastSeenSeq, setLastSeenSeq] = useState(persistedState.lastSeenSeq);
  const [logPath, setLogPath] = useState<string | null>(null);
  const [outputPath, setOutputPath] = useState<string | null>(null);
  const [bulkSession, setBulkSession] = useState<PersistedBulkSession | null>(
    persistedState.bulkSession,
  );
  const [status, setStatus] = useState<RunPocStatus>("idle");
  const [statusMessage, setStatusMessage] = useState(
    "Select one file for SSE drill-down or multiple files for bounded bulk upload and polling.",
  );
  const [logLines, setLogLines] = useState([
    "Temporary ADE run proof of concept.",
    "Single-file runs stream over SSE. Multi-file runs upload in parallel and poll for status.",
  ]);

  useEffect(() => {
    bulkSessionRef.current = bulkSession;
  }, [bulkSession]);

  useEffect(() => {
    window.sessionStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        activeRunScope,
        bulkSession,
        configVersionId,
        lastSeenSeq,
        runId,
        workspaceId,
      } satisfies PersistedState),
    );
  }, [
    activeRunScope,
    bulkSession,
    configVersionId,
    lastSeenSeq,
    runId,
    workspaceId,
  ]);

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

  useEffect(() => {
    if (bulkSession?.batchId === undefined) {
      return;
    }

    let disposed = false;

    async function pollBulkRuns() {
      const current = bulkSessionRef.current;
      if (disposed || !current || bulkPollInFlightRef.current) {
        return;
      }

      const pendingItems = current.items.filter(shouldPollBulkItem);
      if (pendingItems.length === 0) {
        const hasFailures = current.items.some(
          (item) => item.status === "failed",
        );
        setRunStatus(
          hasFailures ? "error" : "completed",
          hasFailures
            ? "Bulk run finished with failures."
            : "Bulk run finished successfully.",
        );
        return;
      }

      bulkPollInFlightRef.current = true;
      try {
        const updates = await Promise.all(
          pendingItems.map(async (item) => {
            const response = await fetch(
              buildRunPath(current.scope, item.runId ?? ""),
            );
            if (!response.ok) {
              return {
                errorMessage: await readErrorMessage(response),
                fileId: item.fileId,
                logPath: item.logPath,
                outputPath: item.outputPath,
                status: "failed" as const,
              };
            }

            const detail = (await response.json()) as RunDetail;
            const nextStatus = mapRunStatusToBulkStatus(detail.status);
            return {
              errorMessage: detail.errorMessage ?? null,
              fileId: item.fileId,
              logPath: detail.logPath ?? item.logPath,
              outputPath: detail.outputPath ?? item.outputPath,
              status: nextStatus,
            };
          }),
        );

        const nextItems = current.items.map((item) => {
          const update = updates.find(
            (candidate) => candidate.fileId === item.fileId,
          );
          if (!update) {
            return item;
          }

          if (item.status !== update.status) {
            appendLogLine(
              `[bulk/run/${item.fileName}] ${item.status} -> ${update.status}`,
            );
          }

          return {
            ...item,
            errorMessage: update.errorMessage,
            logPath: update.logPath,
            outputPath: update.outputPath,
            status: update.status,
          };
        });
        setBulkSession((previous) => {
          if (previous?.batchId !== current.batchId) {
            return previous;
          }

          return {
            ...previous,
            items: nextItems,
          };
        });

        const hasFailures = nextItems.some((item) => item.status === "failed");
        const allTerminal = nextItems.every((item) =>
          isBulkTerminalStatus(item.status),
        );
        if (allTerminal) {
          setRunStatus(
            hasFailures ? "error" : "completed",
            hasFailures
              ? "Bulk run finished with failures."
              : "Bulk run finished successfully.",
          );
        } else if (statusRef.current !== "error") {
          setRunStatus(
            "starting",
            "Bulk upload finished. Polling run progress every 5 seconds.",
          );
        }
      } finally {
        bulkPollInFlightRef.current = false;
      }
    }

    void pollBulkRuns();
    const intervalId = window.setInterval(() => {
      void pollBulkRuns();
    }, BULK_POLL_INTERVAL_MS);

    return () => {
      disposed = true;
      window.clearInterval(intervalId);
    };
  }, [bulkSession?.batchId]);

  function currentScope(): RunScope {
    return {
      configVersionId,
      workspaceId,
    };
  }

  function currentRunScope(): RunScope {
    return activeRunScope ?? currentScope();
  }

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

  function parseEventData<T>(data: string): T | null {
    let payload: unknown;
    try {
      payload = JSON.parse(data);
    } catch {
      return null;
    }

    return payload as T;
  }

  function addRunEventListener<T>(
    source: EventSource,
    eventName: string,
    invalidMessage: string,
    onPayload: (payload: T) => void,
  ) {
    source.addEventListener(eventName, (event) => {
      if (!isActiveEventSource(source)) {
        return;
      }

      const message = event as MessageEvent<string>;
      noteSeq(message.lastEventId);
      const payload = parseEventData<T>(message.data);
      if (!payload) {
        setRunStatus("error", invalidMessage);
        return;
      }

      onPayload(payload);
    });
  }

  function connectToRun(
    targetRunId: string,
    after: number | null,
    scope: RunScope,
  ) {
    closeEventSource();
    setActiveRunScope(scope);
    setRunStatus("connecting", "Connecting to the run event stream...");

    const source = new EventSource(buildEventsPath(scope, targetRunId, after));
    eventSourceRef.current = source;

    source.addEventListener("open", () => {
      if (!isActiveEventSource(source)) {
        return;
      }
      appendLogLine(`[run] connected to ${targetRunId}`);
    });

    addRunEventListener<RunCreatedEvent>(
      source,
      "run.created",
      "Received an invalid run.created payload.",
      (payload) => {
        setRunId(payload.runId);
        setRunStatus("streaming", `Run ${payload.runId} created.`);
        appendLogLine(`[run] created ${payload.runId} (${payload.status})`);
      },
    );

    addRunEventListener<RunStatusEvent>(
      source,
      "run.status",
      "Received an invalid run.status payload.",
      (payload) => {
        setRunStatus("streaming", `Run phase: ${payload.phase}.`);
        appendLogLine(`[${payload.phase}] ${payload.state}`);
      },
    );

    addRunEventListener<RunLogEvent>(
      source,
      "run.log",
      "Received an invalid run.log payload.",
      (payload) => {
        appendLogLine(`[${payload.phase}/${payload.level}] ${payload.message}`);
      },
    );

    addRunEventListener<RunErrorEvent>(
      source,
      "run.error",
      "Received an invalid run.error payload.",
      (payload) => {
        setRunStatus("error", payload.message);
        appendLogLine(
          `[error${payload.phase ? `/${payload.phase}` : ""}] ${payload.message}`,
        );
      },
    );

    addRunEventListener<RunResultEvent>(
      source,
      "run.result",
      "Received an invalid run.result payload.",
      (payload) => {
        setOutputPath(payload.outputPath);
        appendLogLine(`[result] ${payload.outputPath}`);
      },
    );

    addRunEventListener<RunCompletedEvent>(
      source,
      "run.completed",
      "Received an invalid run.completed payload.",
      (payload) => {
        setLogPath(payload.logPath ?? null);
        setOutputPath(payload.outputPath ?? null);
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

  async function createRun(scope: RunScope, inputPath: string) {
    const response = await fetch(
      buildRunsPath(scope.workspaceId, scope.configVersionId),
      {
        body: JSON.stringify({
          inputPath,
        }),
        headers: {
          "Content-Type": "application/json",
        },
        method: "POST",
      },
    );

    if (!response.ok) {
      throw new Error(await readErrorMessage(response));
    }

    return (await response.json()) as {
      runId: string;
      status: string;
    };
  }

  async function downloadArtifact(kind: "log" | "output") {
    if (!runId) {
      setRunStatus("error", "Enter a run ID before downloading artifacts.");
      return;
    }

    const scope = currentRunScope();
    const response = await fetch(buildDownloadsPath(scope, runId), {
      body: JSON.stringify({
        artifact: kind,
      }),
      headers: {
        "Content-Type": "application/json",
      },
      method: "POST",
    });

    if (!response.ok) {
      setRunStatus("error", await readErrorMessage(response));
      return;
    }

    const payload = (await response.json()) as {
      download: ArtifactAccessInstruction;
      filePath: string;
    };
    const artifactResponse = await fetch(payload.download.url, {
      headers: new Headers(payload.download.headers),
      method: payload.download.method,
    });

    if (!artifactResponse.ok) {
      setRunStatus("error", await readErrorMessage(artifactResponse));
      return;
    }

    const blob = await artifactResponse.blob();
    const objectUrl = window.URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = objectUrl;
    anchor.download = payload.filePath.split("/").pop() ?? `${kind}.bin`;
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
    window.URL.revokeObjectURL(objectUrl);

    if (kind === "log") {
      setLogPath(payload.filePath);
    } else {
      setOutputPath(payload.filePath);
    }
    appendLogLine(`[download/${kind}] ${payload.filePath}`);
  }

  function updateBulkItem(
    fileId: string,
    update: (item: BulkItem) => BulkItem,
  ) {
    setBulkSession((previous) => {
      if (previous === null) {
        return previous;
      }

      return {
        ...previous,
        items: previous.items.map((item) =>
          item.fileId === fileId ? update(item) : item,
        ),
      };
    });
  }

  function applyScope(nextScope: RunScope) {
    if (
      nextScope.workspaceId === workspaceId &&
      nextScope.configVersionId === configVersionId
    ) {
      return;
    }

    closeEventSource();
    setWorkspaceId(nextScope.workspaceId);
    setConfigVersionId(nextScope.configVersionId);
    setActiveRunScope(null);
    setBulkSession(null);
    setRunId("");
    setLastSeenSeq(null);
    setLogPath(null);
    setOutputPath(null);
    setRunStatus(
      "idle",
      "Scope changed. Select files or enter a run ID to continue.",
    );
  }

  async function startSingleRun(file: File) {
    const scope = currentScope();
    closeEventSource();
    setBulkSession(null);
    setActiveRunScope(scope);
    setRunId("");
    setLastSeenSeq(null);
    setLogPath(null);
    setOutputPath(null);
    setLogLines([
      "Temporary ADE run proof of concept.",
      `Selected input: ${file.name}`,
    ]);

    setRunStatus("uploading", "Requesting upload instructions...");
    const uploadResponse = await fetch(
      buildUploadsPath(scope.workspaceId, scope.configVersionId),
      {
        body: JSON.stringify({
          contentType: file.type || undefined,
          filename: file.name,
        }),
        headers: {
          "Content-Type": "application/json",
        },
        method: "POST",
      },
    );

    if (!uploadResponse.ok) {
      setRunStatus("error", await readErrorMessage(uploadResponse));
      return;
    }

    const uploadPayload = (await uploadResponse.json()) as {
      filePath: string;
      upload: ArtifactAccessInstruction;
    };
    appendLogLine(`[upload] reserved ${uploadPayload.filePath}`);

    try {
      await uploadToStorage(file, uploadPayload.upload, () => undefined);
    } catch (error) {
      setRunStatus(
        "error",
        error instanceof Error ? error.message : "Direct upload failed.",
      );
      return;
    }

    appendLogLine(`[upload] completed ${uploadPayload.filePath}`);
    setRunStatus("starting", "Creating run...");

    let payload: { runId: string; status: string };
    try {
      payload = await createRun(scope, uploadPayload.filePath);
    } catch (error) {
      setRunStatus(
        "error",
        error instanceof Error ? error.message : "Failed to create the run.",
      );
      return;
    }

    setRunId(payload.runId);
    appendLogLine(`[run] accepted ${payload.runId} (${payload.status})`);
    connectToRun(payload.runId, null, scope);
  }

  async function startBulkRun(files: File[]) {
    const scope = currentScope();
    closeEventSource();
    setActiveRunScope(null);
    setRunId("");
    setLastSeenSeq(null);
    setLogPath(null);
    setOutputPath(null);
    setLogLines([
      "Temporary ADE run proof of concept.",
      `Selected ${files.length} input files for bulk upload.`,
    ]);

    setRunStatus("uploading", "Requesting bulk upload instructions...");
    const response = await fetch(
      buildUploadBatchesPath(scope.workspaceId, scope.configVersionId),
      {
        body: JSON.stringify({
          files: files.map((file) => ({
            contentType: file.type || undefined,
            filename: file.name,
            size: file.size,
          })),
        }),
        headers: {
          "Content-Type": "application/json",
        },
        method: "POST",
      },
    );

    if (!response.ok) {
      setRunStatus("error", await readErrorMessage(response));
      return;
    }

    const payload = (await response.json()) as BulkUploadBatchResponse;
    if (payload.items.length !== files.length) {
      setRunStatus(
        "error",
        "Bulk upload response did not match the selected files.",
      );
      return;
    }

    const reservedItems = files.map((file, index) => {
      const reserved = payload.items[index];
      if (!reserved) {
        throw new Error(
          "Bulk upload response did not match the selected files.",
        );
      }

      return { file, reserved };
    });
    const items = reservedItems.map(
      ({ file, reserved }): BulkItem => ({
        errorMessage: null,
        fileId: reserved.fileId,
        fileName: file.name,
        filePath: reserved.filePath,
        logPath: null,
        outputPath: null,
        runId: null,
        size: file.size,
        status: "selected",
        uploadedBytes: 0,
      }),
    );
    setBulkSession({
      batchId: payload.batchId,
      items,
      scope,
    });
    appendLogLine(`[bulk] reserved ${payload.batchId} (${files.length} files)`);

    for (
      let index = 0;
      index < reservedItems.length;
      index += BULK_UPLOAD_FILE_CONCURRENCY
    ) {
      await Promise.all(
        reservedItems
          .slice(index, index + BULK_UPLOAD_FILE_CONCURRENCY)
          .map(async ({ file, reserved }) => {
            updateBulkItem(reserved.fileId, (item) => ({
              ...item,
              errorMessage: null,
              status: "uploading",
              uploadedBytes: 0,
            }));
            appendLogLine(
              `[bulk/upload/${file.name}] reserved ${reserved.filePath}`,
            );

            try {
              await uploadToStorage(file, reserved.upload, (loadedBytes) => {
                updateBulkItem(reserved.fileId, (item) => ({
                  ...item,
                  status: "uploading",
                  uploadedBytes: loadedBytes,
                }));
              });
              updateBulkItem(reserved.fileId, (item) => ({
                ...item,
                status: "uploaded",
                uploadedBytes: item.size,
              }));
              appendLogLine(`[bulk/upload/${file.name}] completed`);
            } catch (error) {
              updateBulkItem(reserved.fileId, (item) => ({
                ...item,
                errorMessage:
                  error instanceof Error
                    ? error.message
                    : "Direct upload failed.",
                status: "failed",
              }));
              appendLogLine(`[bulk/upload/${file.name}] failed`);
              return;
            }

            try {
              const run = await createRun(scope, reserved.filePath);
              updateBulkItem(reserved.fileId, (item) => ({
                ...item,
                runId: run.runId,
                status: run.status === "running" ? "running" : "runPending",
              }));
              appendLogLine(`[bulk/run/${file.name}] accepted ${run.runId}`);
            } catch (error) {
              updateBulkItem(reserved.fileId, (item) => ({
                ...item,
                errorMessage:
                  error instanceof Error
                    ? error.message
                    : "Failed to create the run.",
                status: "failed",
              }));
              appendLogLine(`[bulk/run/${file.name}] failed to create`);
            }
          }),
      );
    }

    const current = bulkSessionRef.current;
    if (!current) {
      return;
    }

    const hasFailures = current.items.some((item) => item.status === "failed");
    setRunStatus(
      hasFailures ? "error" : "starting",
      hasFailures
        ? "Some bulk uploads or run requests failed. Polling the rest."
        : "Uploads finished. Polling run progress every 5 seconds.",
    );
  }

  async function startRun() {
    const files = Array.from(fileInputRef.current?.files ?? []);
    if (files.length === 0) {
      setRunStatus("error", "Choose at least one input file before starting.");
      return;
    }

    if (files.length === 1) {
      const [file] = files;
      if (file) {
        await startSingleRun(file);
      }
      return;
    }

    await startBulkRun(files);
  }

  async function cancelRun() {
    if (!runId) {
      setRunStatus("error", "Enter a run ID before cancelling.");
      return;
    }

    const response = await fetch(buildCancelPath(currentRunScope(), runId), {
      method: "POST",
    });
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
          Direct upload, async runs, and bounded bulk ingestion.
        </h2>
        <p className="hero__summary">
          This page validates the public split: the browser negotiates
          short-lived storage access, uploads directly to storage, creates runs
          over HTTP, and either streams one run over
          <code>{"/runs/{runId}/events"}</code> or polls many runs after a bulk
          upload batch.
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
                applyScope({
                  configVersionId,
                  workspaceId: event.target.value,
                });
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
                applyScope({
                  configVersionId: event.target.value,
                  workspaceId,
                });
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
            <span>Input Files</span>
            <input
              className="terminal-poc__input"
              multiple
              name="inputFile"
              ref={fileInputRef}
              type="file"
            />
          </label>

          <div className="terminal-poc__actions run-poc__actions">
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
                connectToRun(runId, lastSeenSeq, currentRunScope());
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
            <button
              className="terminal-poc__button terminal-poc__button--secondary"
              onClick={() => {
                void downloadArtifact("output");
              }}
              type="button"
            >
              Download Output
            </button>
            <button
              className="terminal-poc__button terminal-poc__button--secondary"
              onClick={() => {
                void downloadArtifact("log");
              }}
              type="button"
            >
              Download Log
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
              {runId || "Not selected"}
            </p>
          </section>
          <section className="status-card">
            <p className="status-card__label">Last Event</p>
            <p className="status-card__value status-card__value--mono">
              {lastSeenSeq ?? "None"}
            </p>
          </section>
          <section className="status-card">
            <p className="status-card__label">Batch</p>
            <p className="status-card__value status-card__value--mono">
              {bulkSession?.batchId ?? "None"}
            </p>
          </section>
          <section className="status-card">
            <p className="status-card__label">Output</p>
            <p className="status-card__value status-card__value--mono">
              {outputPath ?? "Pending"}
            </p>
          </section>
          <section className="status-card">
            <p className="status-card__label">Log</p>
            <p className="status-card__value status-card__value--mono">
              {logPath ?? "Pending"}
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

        {bulkSession && (
          <section className="run-poc__batch">
            <div className="run-poc__batch-header">
              <p className="status-card__label">Bulk Session</p>
              <p className="status-card__value status-card__value--mono">
                {bulkSession.scope.workspaceId}/
                {bulkSession.scope.configVersionId}
              </p>
            </div>
            <div className="run-poc__batch-list">
              {bulkSession.items.map((item) => (
                <article className="run-poc__batch-item" key={item.fileId}>
                  <div className="run-poc__batch-main">
                    <p className="run-poc__batch-file">{item.fileName}</p>
                    <p className="run-poc__batch-meta status-card__value--mono">
                      {item.filePath}
                    </p>
                  </div>
                  <div className="run-poc__batch-stats">
                    <span>{item.status}</span>
                    <span>{bulkProgressLabel(item)}</span>
                    <span className="status-card__value--mono">
                      {item.runId ?? "No run"}
                    </span>
                  </div>
                  {item.errorMessage && (
                    <p className="status-note status-note--error">
                      {item.errorMessage}
                    </p>
                  )}
                </article>
              ))}
            </div>
          </section>
        )}
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
