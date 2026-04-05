import {
  expect,
  type APIRequestContext,
  type APIResponse,
} from "@playwright/test";

type Scope = {
  configVersionId: string;
  workspaceId: string;
};

type ArtifactInstruction = {
  headers: Record<string, string>;
  method: string;
  url: string;
};

type RunDetail = {
  errorMessage?: string | null;
  outputPath?: string | null;
  runId: string;
  status: string;
  validationIssues: unknown[];
};

type CompletedRun = {
  outputPath: string;
  runId: string;
};

const primaryScope: Scope = {
  configVersionId: "config-v1",
  workspaceId: "workspace-a",
};

const otherScope: Scope = {
  configVersionId: "config-v2",
  workspaceId: "workspace-b",
};

function acceptanceBaseUrl(): string {
  const baseUrl = process.env["PLAYWRIGHT_BASE_URL"]
    ?.trim()
    .replace(/\/+$/, "");

  if (!baseUrl) {
    throw new Error(
      "PLAYWRIGHT_BASE_URL is required. Use `pnpm test:acceptance --url <base-url>` or `pnpm test:acceptance --image <image>`.",
    );
  }

  return baseUrl;
}

function scopePath(scope: Scope, suffix = ""): string {
  return `/api/workspaces/${encodeURIComponent(scope.workspaceId)}/configs/${encodeURIComponent(scope.configVersionId)}${suffix}`;
}

async function expectJson(
  response: APIResponse,
  description: string,
  expectedStatus = 200,
): Promise<unknown> {
  expect(
    response.status(),
    `${description} should return ${String(expectedStatus)}.`,
  ).toBe(expectedStatus);
  return response.json();
}

async function sendArtifactRequest(
  request: APIRequestContext,
  instruction: ArtifactInstruction,
  body?: Uint8Array,
): Promise<APIResponse> {
  return request.fetch(
    new URL(instruction.url, acceptanceBaseUrl()).toString(),
    {
      data: body ? Buffer.from(body) : undefined,
      failOnStatusCode: false,
      headers: instruction.headers,
      method: instruction.method,
    },
  );
}

async function uploadCsv(
  request: APIRequestContext,
  scope: Scope,
  filename: string,
  content: string,
): Promise<string> {
  const uploadResponse = (await expectJson(
    await request.post(scopePath(scope, "/uploads"), {
      data: {
        contentType: "text/csv",
        filename,
      },
    }),
    "upload creation",
  )) as {
    filePath: string;
    upload: ArtifactInstruction;
  };

  const directUpload = await sendArtifactRequest(
    request,
    uploadResponse.upload,
    new TextEncoder().encode(content),
  );

  expect(
    directUpload.ok(),
    `Direct upload for ${filename} should succeed.`,
  ).toBeTruthy();

  return uploadResponse.filePath;
}

async function startRun(
  request: APIRequestContext,
  scope: Scope,
  inputPath: string,
): Promise<string> {
  const created = (await expectJson(
    await request.post(scopePath(scope, "/runs"), {
      data: { inputPath },
      failOnStatusCode: false,
    }),
    "run creation",
    202,
  )) as {
    runId: string;
    status: string;
  };

  expect(created.status, "Run should start in the pending state.").toBe(
    "pending",
  );

  return created.runId;
}

async function waitForCompletedRun(
  request: APIRequestContext,
  scope: Scope,
  runId: string,
): Promise<CompletedRun> {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const detail = (await expectJson(
      await request.get(scopePath(scope, `/runs/${encodeURIComponent(runId)}`)),
      "run detail",
    )) as RunDetail;

    if (detail.status === "succeeded") {
      expect(
        detail.outputPath,
        "Completed runs should expose a downloadable output path.",
      ).toBeTruthy();
      expect(
        detail.validationIssues,
        "Acceptance fixtures should not produce validation issues.",
      ).toEqual([]);

      return {
        outputPath: detail.outputPath ?? "",
        runId: detail.runId,
      };
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

async function completeCsvRun(
  request: APIRequestContext,
  scope: Scope,
  filename: string,
  content: string,
): Promise<CompletedRun> {
  const inputPath = await uploadCsv(request, scope, filename, content);
  const runId = await startRun(request, scope, inputPath);
  return waitForCompletedRun(request, scope, runId);
}

async function downloadRunArtifact(
  request: APIRequestContext,
  scope: Scope,
  runId: string,
  artifact: "log" | "output",
): Promise<{ bytes: Buffer; filePath: string }> {
  const downloadResponse = (await expectJson(
    await request.post(
      scopePath(scope, `/runs/${encodeURIComponent(runId)}/downloads`),
      {
        data: { artifact },
      },
    ),
    `${artifact} download creation`,
  )) as {
    download: ArtifactInstruction;
    filePath: string;
  };

  const artifactResponse = await sendArtifactRequest(
    request,
    downloadResponse.download,
  );
  expect(
    artifactResponse.ok(),
    `${artifact} download should succeed.`,
  ).toBeTruthy();

  return {
    bytes: await artifactResponse.body(),
    filePath: downloadResponse.filePath,
  };
}

async function expectRunHiddenFromScope(
  request: APIRequestContext,
  scope: Scope,
  runId: string,
): Promise<void> {
  expect(
    (await request.get(scopePath(scope, `/runs/${encodeURIComponent(runId)}`)))
      .status(),
    "Cross-scope run detail should return 404.",
  ).toBe(404);

  expect(
    (
      await request.post(
        scopePath(scope, `/runs/${encodeURIComponent(runId)}/downloads`),
        {
          data: { artifact: "output" },
        },
      )
    ).status(),
    "Cross-scope run download should return 404.",
  ).toBe(404);
}

async function expectApiReady(request: APIRequestContext): Promise<void> {
  const health = (await expectJson(
    await request.get("/api/healthz"),
    "API health check",
  )) as {
    service?: unknown;
    status?: unknown;
  };
  const ready = (await expectJson(
    await request.get("/api/readyz"),
    "API readiness check",
  )) as {
    service?: unknown;
    status?: unknown;
  };
  const version = (await expectJson(
    await request.get("/api/version"),
    "API version check",
  )) as {
    service?: unknown;
    version?: unknown;
  };
  const apiRoot = (await expectJson(
    await request.get("/api/"),
    "API root endpoint",
  )) as {
    service?: unknown;
    status?: unknown;
    version?: unknown;
  };

  expect(health.service).toBe("ade");
  expect(health.status).toBe("ok");
  expect(ready.service).toBe("ade");
  expect(ready.status).toBe("ready");
  expect(version.service).toBe("ade");
  expect(typeof version.version).toBe("string");
  expect(apiRoot.service).toBe("ade");
  expect(apiRoot.status).toBe("ok");
  expect(typeof apiRoot.version).toBe("string");
}

export {
  completeCsvRun,
  downloadRunArtifact,
  expectApiReady,
  expectRunHiddenFromScope,
  otherScope,
  primaryScope,
};

export type { Scope };
