import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { apiClient } from "../api/client";
import type { components } from "../api/schema";

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

type ArtifactAccessInstruction =
  components["schemas"]["ArtifactAccessInstruction"];
type RunDetailResponse = components["schemas"]["RunDetailResponse"];
type RunStatus = components["schemas"]["RunStatus"];

type RunCreatedEvent = {
  runId: string;
  status: RunStatus;
};

type RunStatusEvent = {
  phase: string;
  state: string;
};

type RunLogEvent = {
  level: string;
  message: string;
  phase: string;
};

type RunErrorEvent = {
  message: string;
  phase?: string | null;
};

type RunResultEvent = {
  outputPath: string;
};

type RunCompletedEvent = {
  finalStatus: RunStatus;
  logPath?: string | null;
  outputPath?: string | null;
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

function buildEventsPath(
  scope: RunScope,
  runId: string,
  after: number | null,
): string {
  const url = new URL(
    `/api/workspaces/${encodeURIComponent(scope.workspaceId)}/configs/${encodeURIComponent(scope.configVersionId)}/runs/${encodeURIComponent(runId)}/events`,
    window.location.origin,
  );
  if (after !== null) {
    url.searchParams.set("after", String(after));
  }
  return url.toString();
}

function mapRunStatusToBulkStatus(status: RunStatus): BulkItemStatus {
  switch (status) {
    case "pending":
      return "runPending";
    case "running":
      return "running";
    case "succeeded":
      return "succeeded";
    case "failed":
    case "cancelled":
      return "failed";
  }
}

function isRunTerminalStatus(status: RunStatus): boolean {
  return status === "cancelled" || status === "failed" || status === "succeeded";
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
  return `${String(Math.max(0, Math.min(percent, 100)))}%`;
}

async function readErrorMessage(response: Response): Promise<string> {
  const fallback = `Request failed with ${String(response.status)}.`;
  const text = await response.text();
  if (!text) {
    return fallback;
  }

  try {
    const payload = JSON.parse(text) as {
      message?: string;
      error?: string;
    };
    return payload.message ?? payload.error ?? text;
  } catch {
    return text;
  }
}

async function uploadToStorage(
  file: File,
  upload: ArtifactAccessInstruction,
  onProgress: (loadedBytes: number) => void,
) {
  const headers = new Headers(upload.headers);
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
  const bulkSessionRef = useRef(persistedState.bulkSession);
  const [workspaceId, setWorkspaceId] = useState(persistedState.workspaceId);
  const [configVersionId, setConfigVersionId] = useState(
    persistedState.configVersionId,
  );
  const [runId, setRunId] = useState(persistedState.runId);
  const [activeRunScope, setActiveRunScope] = useState(
    persistedState.activeRunScope,
  );
  const [lastSeenSeq, setLastSeenSeq] = useState(persistedState.lastSeenSeq);
  const [logPath, setLogPath] = useState<string | null>(null);
  const [outputPath, setOutputPath] = useState<string | null>(null);
  const [bulkSession, setBulkSession] = useState(persistedState.bulkSession);
  const [status, setStatus] = useState<RunPocStatus>("idle");
  const [statusMessage, setStatusMessage] = useState(
    "Select one file for SSE drill-down or multiple files for bounded bulk upload and polling.",
  );
  const [logLines, setLogLines] = useState([
    "Temporary ADE run proof of concept.",
    "Single-file runs stream over SSE. Multi-file runs upload in parallel and poll for status.",
  ]);
  const bulkInProgress =
    bulkSession?.items.some((item) => !isBulkTerminalStatus(item.status)) ??
    false;
  const formLocked =
    bulkInProgress ||
    status === "uploading" ||
    status === "starting" ||
    status === "connecting" ||
    status === "streaming";

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
    if (!bulkSession) {
      return;
    }

    let cancelled = false;
    let timeoutId: number | null = null;

    async function pollBulkRuns() {
      const current = bulkSessionRef.current;
      if (cancelled || !current) {
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

      let updates;
      try {
        updates = await Promise.all(
          pendingItems.map(async (item) => {
            if (!item.runId) {
              return {
                errorMessage: "Run is missing an identifier.",
                fileId: item.fileId,
                logPath: item.logPath,
                outputPath: item.outputPath,
                status: "failed" as const,
              };
            }

            const { data } = await apiClient.GET(
              "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}",
              {
                params: {
                  path: {
                    configVersionId: current.scope.configVersionId,
                    runId: item.runId,
                    workspaceId: current.scope.workspaceId,
                  },
                },
              },
            );
            if (!data) {
              return {
                errorMessage: "Failed to load run status.",
                fileId: item.fileId,
                logPath: item.logPath,
                outputPath: item.outputPath,
                status: "failed" as const,
              };
            }

            const detail = data;
            return {
              errorMessage: detail.errorMessage ?? null,
              fileId: item.fileId,
              logPath: detail.logPath ?? item.logPath,
              outputPath: detail.outputPath ?? item.outputPath,
              status: mapRunStatusToBulkStatus(detail.status),
            };
          }),
        );
      } catch (error) {
        setRunStatus(
          "error",
          error instanceof Error ? error.message : "Failed to poll bulk runs.",
        );
        return;
      }

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
        return;
      }

      if (statusRef.current !== "error") {
        setRunStatus(
          "starting",
          "Bulk upload finished. Polling run progress every 5 seconds.",
        );
      }

      timeoutId = window.setTimeout(() => {
        void pollBulkRuns();
      }, BULK_POLL_INTERVAL_MS);
    }

    void pollBulkRuns();

    return () => {
      cancelled = true;
      if (timeoutId !== null) {
        window.clearTimeout(timeoutId);
      }
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

  function isActiveEventSource(source: EventSource): boolean {
    return eventSourceRef.current === source;
  }

  function appendLogLine(line: string) {
    setLogLines((previous) => {
      const next = [...previous, line];
      return next.length > 800 ? next.slice(next.length - 800) : next;
    });
  }

  function closeEventSource() {
    eventSourceRef.current?.close();
    eventSourceRef.current = null;
  }

  function noteSeq(value: string | null) {
    if (!value?.trim()) {
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

  function addRunEventListener(
    source: EventSource,
    eventName: string,
    invalidMessage: string,
    onPayload: (payload: unknown) => void,
  ) {
    source.addEventListener(eventName, (event) => {
      if (!isActiveEventSource(source)) {
        return;
      }

      const message = event as MessageEvent<string>;
      noteSeq(message.lastEventId);
      let payload: unknown;
      try {
        payload = JSON.parse(message.data) as unknown;
      } catch {
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

    addRunEventListener(
      source,
      "run.created",
      "Received an invalid run.created payload.",
      (payload) => {
        const created = payload as RunCreatedEvent;
        setRunId(created.runId);
        setRunStatus("streaming", `Run ${created.runId} created.`);
        appendLogLine(`[run] created ${created.runId} (${created.status})`);
      },
    );

    addRunEventListener(
      source,
      "run.status",
      "Received an invalid run.status payload.",
      (payload) => {
        const statusEvent = payload as RunStatusEvent;
        setRunStatus("streaming", `Run phase: ${statusEvent.phase}.`);
        appendLogLine(`[${statusEvent.phase}] ${statusEvent.state}`);
      },
    );

    addRunEventListener(
      source,
      "run.log",
      "Received an invalid run.log payload.",
      (payload) => {
        const logEvent = payload as RunLogEvent;
        appendLogLine(
          `[${logEvent.phase}/${logEvent.level}] ${logEvent.message}`,
        );
      },
    );

    addRunEventListener(
      source,
      "run.error",
      "Received an invalid run.error payload.",
      (payload) => {
        const errorEvent = payload as RunErrorEvent;
        setRunStatus("error", errorEvent.message);
        appendLogLine(
          `[error${errorEvent.phase ? `/${errorEvent.phase}` : ""}] ${errorEvent.message}`,
        );
      },
    );

    addRunEventListener(
      source,
      "run.result",
      "Received an invalid run.result payload.",
      (payload) => {
        const resultEvent = payload as RunResultEvent;
        setOutputPath(resultEvent.outputPath);
        appendLogLine(`[result] ${resultEvent.outputPath}`);
      },
    );

    addRunEventListener(
      source,
      "run.completed",
      "Received an invalid run.completed payload.",
      (payload) => {
        const completed = payload as RunCompletedEvent;
        setLogPath(completed.logPath ?? null);
        setOutputPath(completed.outputPath ?? null);
        setRunStatus("completed", `Run ${completed.finalStatus}.`);
        appendLogLine(`[complete] ${completed.finalStatus}`);
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
    const { data } = await apiClient.POST(
      "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs",
      {
        body: {
          inputPath,
        },
        params: {
          path: {
            configVersionId: scope.configVersionId,
            workspaceId: scope.workspaceId,
          },
        },
      },
    );
    if (!data) {
      throw new Error("Failed to create the run.");
    }

    return data;
  }

  async function loadRunDetail(
    scope: RunScope,
    targetRunId: string,
  ): Promise<RunDetailResponse> {
    const { data } = await apiClient.GET(
      "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}",
      {
        params: {
          path: {
            configVersionId: scope.configVersionId,
            runId: targetRunId,
            workspaceId: scope.workspaceId,
          },
        },
      },
    );
    if (!data) {
      throw new Error("Failed to load run details.");
    }

    return data;
  }

  async function downloadArtifact(kind: "log" | "output") {
    if (!runId) {
      setRunStatus("error", "Enter a run ID before downloading artifacts.");
      return;
    }

    try {
      const scope = currentRunScope();
      const { data } = await apiClient.POST(
        "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/downloads",
        {
          body: {
            artifact: kind,
          },
          params: {
            path: {
              configVersionId: scope.configVersionId,
              runId,
              workspaceId: scope.workspaceId,
            },
          },
        },
      );
      if (!data) {
        setRunStatus("error", "Failed to create download link.");
        return;
      }

      const artifactResponse = await fetch(data.download.url, {
        headers: new Headers(data.download.headers),
        method: data.download.method,
      });

      if (!artifactResponse.ok) {
        setRunStatus("error", await readErrorMessage(artifactResponse));
        return;
      }

      const blob = await artifactResponse.blob();
      const objectUrl = window.URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = objectUrl;
      anchor.download = data.filePath.split("/").pop() ?? `${kind}.bin`;
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
      window.URL.revokeObjectURL(objectUrl);

      if (kind === "log") {
        setLogPath(data.filePath);
      } else {
        setOutputPath(data.filePath);
      }
      appendLogLine(`[download/${kind}] ${data.filePath}`);
    } catch (error) {
      setRunStatus(
        "error",
        error instanceof Error ? error.message : "Failed to download artifact.",
      );
    }
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
    let uploadPayload;
    try {
      const { data } = await apiClient.POST(
        "/api/workspaces/{workspaceId}/configs/{configVersionId}/uploads",
        {
          body: {
            contentType: file.type || null,
            filename: file.name,
          },
          params: {
            path: {
              configVersionId: scope.configVersionId,
              workspaceId: scope.workspaceId,
            },
          },
        },
      );
      if (!data) {
        setRunStatus("error", "Failed to create upload link.");
        return;
      }
      uploadPayload = data;
    } catch (error) {
      setRunStatus(
        "error",
        error instanceof Error ? error.message : "Failed to create upload link.",
      );
      return;
    }

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

    try {
      const payload = await createRun(scope, uploadPayload.filePath);
      setRunId(payload.runId);
      appendLogLine(`[run] accepted ${payload.runId} (${payload.status})`);
      if (isRunTerminalStatus(payload.status)) {
        const detail = await loadRunDetail(scope, payload.runId);
        setLogPath(detail.logPath ?? null);
        setOutputPath(detail.outputPath ?? null);
        setRunStatus(
          payload.status === "succeeded" ? "completed" : "error",
          `Run ${payload.status}.`,
        );
        appendLogLine(`[complete] ${payload.status}`);
        return;
      }
      connectToRun(payload.runId, null, scope);
    } catch (error) {
      setRunStatus(
        "error",
        error instanceof Error ? error.message : "Failed to create the run.",
      );
      return;
    }
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
      `Selected ${String(files.length)} input files for bulk upload.`,
    ]);

    setRunStatus("uploading", "Requesting bulk upload instructions...");
    let payload;
    try {
      const response = await apiClient.POST(
        "/api/workspaces/{workspaceId}/configs/{configVersionId}/uploads/batches",
        {
          body: {
            files: files.map((file) => ({
              contentType: file.type || null,
              filename: file.name,
              size: file.size,
            })),
          },
          params: {
            path: {
              configVersionId: scope.configVersionId,
              workspaceId: scope.workspaceId,
            },
          },
        },
      );
      if (!response.data) {
        setRunStatus("error", "Failed to create bulk upload links.");
        return;
      }
      payload = response.data;
    } catch (error) {
      setRunStatus(
        "error",
        error instanceof Error
          ? error.message
          : "Failed to create bulk upload links.",
      );
      return;
    }
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
    appendLogLine(
      `[bulk] reserved ${payload.batchId} (${String(files.length)} files)`,
    );

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
              const detail = isRunTerminalStatus(run.status)
                ? await loadRunDetail(scope, run.runId)
                : null;
              updateBulkItem(reserved.fileId, (item) => ({
                ...item,
                errorMessage: detail?.errorMessage ?? item.errorMessage,
                logPath: detail?.logPath ?? item.logPath,
                outputPath: detail?.outputPath ?? item.outputPath,
                runId: run.runId,
                status: mapRunStatusToBulkStatus(run.status),
              }));
              appendLogLine(
                `[bulk/run/${file.name}] accepted ${run.runId} (${run.status})`,
              );
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

    try {
      const scope = currentRunScope();
      const { error } = await apiClient.POST(
        "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/cancel",
        {
          params: {
            path: {
              configVersionId: scope.configVersionId,
              runId,
              workspaceId: scope.workspaceId,
            },
          },
        },
      );
      if (error) {
        setRunStatus("error", error.message);
        return;
      }

      appendLogLine(`[run] cancel requested for ${runId}`);
    } catch (error) {
      setRunStatus(
        "error",
        error instanceof Error ? error.message : "Failed to cancel the run.",
      );
    }
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
              disabled={formLocked}
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
              disabled={formLocked}
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
              disabled={formLocked}
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
              disabled={formLocked}
              multiple
              name="inputFile"
              ref={fileInputRef}
              type="file"
            />
          </label>

          <div className="terminal-poc__actions run-poc__actions">
            <button
              className="terminal-poc__button"
              disabled={formLocked}
              onClick={() => {
                void startRun();
              }}
              type="button"
            >
              Start Run
            </button>
            <button
              className="terminal-poc__button terminal-poc__button--secondary"
              disabled={!runId}
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
              disabled={!runId || bulkInProgress}
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
