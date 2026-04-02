type ArtifactAccessInstruction = {
  expiresAt: string;
  headers: Record<string, string>;
  method: string;
  url: string;
};

type UploadResponse = {
  filePath: string;
  upload: ArtifactAccessInstruction;
};

type RunCreatedResponse = {
  inputPath: string;
  outputPath: string | null;
  runId: string;
  status: string;
};

type RunDetailResponse = {
  errorMessage?: string | null;
  inputPath: string;
  outputPath?: string | null;
  phase?: string | null;
  runId: string;
  status: string;
  validationIssues: unknown[];
};

type DownloadResponse = {
  download: ArtifactAccessInstruction;
  filePath: string;
};

type Scope = {
  configVersionId: string;
  workspaceId: string;
};

type ScopeResult = {
  outputPath: string;
};

function normalizeBaseUrl(value: string): string {
  const trimmed = value.trim();

  if (trimmed === "") {
    throw new Error("--url must not be empty.");
  }

  return trimmed.replace(/\/+$/, "");
}

function assertString(value: unknown, description: string) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`Expected ${description} to be a non-empty string.`);
  }
}

function scopeBasePath(scope: Scope): string {
  return `/api/workspaces/${encodeURIComponent(scope.workspaceId)}/configs/${encodeURIComponent(scope.configVersionId)}`;
}

function scopedPrefix(scope: Scope): string {
  return `workspaces/${scope.workspaceId}/configs/${scope.configVersionId}`;
}

function apiUrl(baseUrl: string, path: string): string {
  return `${baseUrl}${path}`;
}

async function expectStatus(
  url: string,
  description: string,
  expectedStatus: number,
  init?: RequestInit,
): Promise<Response> {
  const response = await fetch(url, init);

  if (response.status !== expectedStatus) {
    throw new Error(
      `Expected ${description} at ${url} to return ${String(expectedStatus)}, received ${String(response.status)}.`,
    );
  }

  return response;
}

async function expectOk(url: string, description: string): Promise<Response> {
  const response = await fetch(url);

  if (!response.ok) {
    throw new Error(
      `Expected ${description} at ${url} to return 200, received ${String(response.status)}.`,
    );
  }

  return response;
}

async function readJson(url: string, description: string): Promise<unknown> {
  const response = await expectOk(url, description);
  return response.json();
}

async function readApiJson<T>(
  baseUrl: string,
  path: string,
  description: string,
  init?: RequestInit,
  expectedStatus = 200,
): Promise<T> {
  const response = await expectStatus(
    apiUrl(baseUrl, path),
    description,
    expectedStatus,
    init,
  );
  const body = await response.text();

  return JSON.parse(body) as T;
}

async function sendArtifactRequest(
  baseUrl: string,
  instruction: ArtifactAccessInstruction,
  body?: Uint8Array,
): Promise<Response> {
  const headers = new Headers();

  for (const [name, value] of Object.entries(instruction.headers)) {
    headers.set(name, value);
  }

  const init: RequestInit = {
    headers,
    method: instruction.method,
  };

  if (body !== undefined) {
    init.body = Buffer.from(body);
  }

  return fetch(new URL(instruction.url, baseUrl).toString(), init);
}

async function createUpload(
  baseUrl: string,
  scope: Scope,
  filename: string,
  contentType: string,
  content: Uint8Array,
): Promise<UploadResponse> {
  const upload = await readApiJson<UploadResponse>(
    baseUrl,
    `${scopeBasePath(scope)}/uploads`,
    "upload creation",
    {
      body: JSON.stringify({
        contentType,
        filename,
      }),
      headers: {
        "content-type": "application/json",
      },
      method: "POST",
    },
  );

  const response = await sendArtifactRequest(baseUrl, upload.upload, content);

  if (!response.ok) {
    throw new Error(
      `Expected direct upload for ${filename} to succeed, received ${String(response.status)}.`,
    );
  }

  return upload;
}

async function createRun(
  baseUrl: string,
  scope: Scope,
  inputPath: string,
): Promise<RunCreatedResponse> {
  return readApiJson<RunCreatedResponse>(
    baseUrl,
    `${scopeBasePath(scope)}/runs`,
    "run creation",
    {
      body: JSON.stringify({
        inputPath,
      }),
      headers: {
        "content-type": "application/json",
      },
      method: "POST",
    },
    202,
  );
}

async function waitForRun(
  baseUrl: string,
  scope: Scope,
  runId: string,
): Promise<RunDetailResponse> {
  const path = `${scopeBasePath(scope)}/runs/${encodeURIComponent(runId)}`;

  for (let attempt = 0; attempt < 80; attempt += 1) {
    const detail = await readApiJson<RunDetailResponse>(
      baseUrl,
      path,
      "run detail",
    );

    if (detail.status === "succeeded") {
      return detail;
    }

    if (detail.status === "failed" || detail.status === "cancelled") {
      throw new Error(
        `Run ${runId} finished with ${detail.status}: ${detail.errorMessage ?? "no error message"}`,
      );
    }

    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  throw new Error(`Run ${runId} did not finish before the acceptance timeout.`);
}

async function fetchRunEvents(
  baseUrl: string,
  scope: Scope,
  runId: string,
  after?: number,
): Promise<string> {
  const url = new URL(
    apiUrl(
      baseUrl,
      `${scopeBasePath(scope)}/runs/${encodeURIComponent(runId)}/events`,
    ),
  );

  if (after !== undefined) {
    url.searchParams.set("after", String(after));
  }

  const response = await fetch(url, {
    headers: {
      accept: "text/event-stream",
    },
  });

  if (!response.ok) {
    throw new Error(
      `Expected run events for ${runId} to succeed, received ${String(response.status)}.`,
    );
  }

  return response.text();
}

function parseSseIds(body: string): number[] {
  return [...body.matchAll(/^id:\s*(\d+)$/gm)].map((match) =>
    Number.parseInt(match[1] ?? "0", 10),
  );
}

async function createDownload(
  baseUrl: string,
  scope: Scope,
  runId: string,
): Promise<DownloadResponse> {
  return readApiJson<DownloadResponse>(
    baseUrl,
    `${scopeBasePath(scope)}/runs/${encodeURIComponent(runId)}/downloads`,
    "output download creation",
    {
      body: JSON.stringify({
        artifact: "output",
      }),
      headers: {
        "content-type": "application/json",
      },
      method: "POST",
    },
  );
}

async function downloadOutput(
  baseUrl: string,
  instruction: ArtifactAccessInstruction,
): Promise<Uint8Array> {
  const response = await sendArtifactRequest(baseUrl, instruction);

  if (!response.ok) {
    throw new Error(
      `Expected output download to succeed, received ${String(response.status)}.`,
    );
  }

  return new Uint8Array(await response.arrayBuffer());
}

async function assertAppShell(baseUrl: string): Promise<void> {
  const response = await expectOk(`${baseUrl}/`, "web application shell");
  const contentType = response.headers.get("content-type") ?? "";

  if (!contentType.includes("text/html")) {
    throw new Error(
      `Expected ${baseUrl}/ to return HTML, received ${contentType || "unknown content type"}.`,
    );
  }

  const body = await response.text();

  if (!body.includes('id="root"')) {
    throw new Error(
      "Expected the application shell to include the root application mount element.",
    );
  }
}

async function assertSpaDeepLink(baseUrl: string): Promise<void> {
  const response = await expectOk(
    `${baseUrl}/documents/example`,
    "SPA deep-link fallback",
  );
  const contentType = response.headers.get("content-type") ?? "";

  if (!contentType.includes("text/html")) {
    throw new Error(
      `Expected SPA deep-link fallback to return HTML, received ${contentType || "unknown content type"}.`,
    );
  }
}

async function assertHealth(baseUrl: string): Promise<void> {
  const payload = (await readJson(
    `${baseUrl}/api/healthz`,
    "API health check",
  )) as {
    service?: unknown;
    status?: unknown;
  };

  if (payload.service !== "ade" || payload.status !== "ok") {
    throw new Error(
      'Expected /api/healthz to report service "ade" with status "ok".',
    );
  }
}

async function assertReady(baseUrl: string): Promise<void> {
  const payload = (await readJson(
    `${baseUrl}/api/readyz`,
    "API readiness check",
  )) as {
    service?: unknown;
    status?: unknown;
  };

  if (payload.service !== "ade" || payload.status !== "ready") {
    throw new Error(
      'Expected /api/readyz to report service "ade" with status "ready".',
    );
  }
}

async function assertVersion(baseUrl: string): Promise<void> {
  const payload = (await readJson(
    `${baseUrl}/api/version`,
    "API version metadata",
  )) as {
    service?: unknown;
    version?: unknown;
  };

  assertString(payload.service, "version payload service");
  assertString(payload.version, "version payload version");

  if (payload.service !== "ade") {
    throw new Error('Expected /api/version to report service "ade".');
  }
}

async function assertApiRoot(baseUrl: string): Promise<void> {
  const payload = (await readJson(`${baseUrl}/api/`, "API root endpoint")) as {
    service?: unknown;
    status?: unknown;
    version?: unknown;
  };

  if (payload.service !== "ade" || payload.status !== "ok") {
    throw new Error('Expected /api/ to report service "ade" with status "ok".');
  }

  assertString(payload.version, "API root version");
}

async function runScopedAcceptanceFlow(
  baseUrl: string,
  scope: Scope,
  filename: string,
  content: string,
): Promise<ScopeResult> {
  const upload = await createUpload(
    baseUrl,
    scope,
    filename,
    "text/csv",
    new TextEncoder().encode(content),
  );

  if (!upload.filePath.startsWith(`${scopedPrefix(scope)}/uploads/upl_`)) {
    throw new Error(
      `${scope.workspaceId}/${scope.configVersionId} upload path was not scoped correctly.`,
    );
  }

  const run = await createRun(baseUrl, scope, upload.filePath);

  if (run.status !== "pending") {
    throw new Error(
      `${scope.workspaceId}/${scope.configVersionId} run was not accepted in the pending state.`,
    );
  }

  const detail = await waitForRun(baseUrl, scope, run.runId);

  if (
    detail.outputPath === undefined ||
    detail.outputPath === null ||
    !detail.outputPath.startsWith(`${scopedPrefix(scope)}/runs/`) ||
    !detail.outputPath.endsWith("/normalized.xlsx")
  ) {
    throw new Error(
      `${scope.workspaceId}/${scope.configVersionId} did not persist the expected output path.`,
    );
  }

  if (detail.validationIssues.length !== 0) {
    throw new Error(
      `${scope.workspaceId}/${scope.configVersionId} returned unexpected validation issues.`,
    );
  }

  const events = await fetchRunEvents(baseUrl, scope, run.runId);

  if (parseSseIds(events).length === 0) {
    throw new Error(
      `${scope.workspaceId}/${scope.configVersionId} run did not emit SSE ids.`,
    );
  }

  const resumedEvents = await fetchRunEvents(baseUrl, scope, run.runId, 2);

  if (parseSseIds(resumedEvents).some((id) => id <= 2)) {
    throw new Error(
      `${scope.workspaceId}/${scope.configVersionId} SSE resume did not honor the requested sequence.`,
    );
  }

  const download = await createDownload(baseUrl, scope, run.runId);
  const outputBytes = await downloadOutput(baseUrl, download.download);

  if (download.filePath !== detail.outputPath) {
    throw new Error(
      `${scope.workspaceId}/${scope.configVersionId} download path did not match the run output path.`,
    );
  }

  if (outputBytes.byteLength === 0) {
    throw new Error(
      `${scope.workspaceId}/${scope.configVersionId} output artifact was empty.`,
    );
  }

  return {
    outputPath: detail.outputPath,
  };
}

async function runAcceptanceChecks(baseUrl: string): Promise<void> {
  const normalizedBaseUrl = normalizeBaseUrl(baseUrl);

  await assertAppShell(normalizedBaseUrl);
  await assertSpaDeepLink(normalizedBaseUrl);
  await assertHealth(normalizedBaseUrl);
  await assertReady(normalizedBaseUrl);
  await assertVersion(normalizedBaseUrl);
  await assertApiRoot(normalizedBaseUrl);

  const first = await runScopedAcceptanceFlow(
    normalizedBaseUrl,
    {
      configVersionId: "config-v1",
      workspaceId: "workspace-a",
    },
    "acceptance-a.csv",
    "name,email\nalice,alice@example.com\n",
  );
  const second = await runScopedAcceptanceFlow(
    normalizedBaseUrl,
    {
      configVersionId: "config-v2",
      workspaceId: "workspace-b",
    },
    "acceptance-b.csv",
    "name,email\nbob,bob@example.com\n",
  );

  if (first.outputPath === second.outputPath) {
    throw new Error("Acceptance runs did not keep output paths isolated.");
  }
}

export { normalizeBaseUrl, runAcceptanceChecks };
