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

async function runAcceptanceChecks(baseUrl: string): Promise<void> {
  const normalizedBaseUrl = normalizeBaseUrl(baseUrl);
  await assertAppShell(normalizedBaseUrl);
  await assertSpaDeepLink(normalizedBaseUrl);
  await assertHealth(normalizedBaseUrl);
  await assertReady(normalizedBaseUrl);
  await assertVersion(normalizedBaseUrl);
  await assertApiRoot(normalizedBaseUrl);
}

export { normalizeBaseUrl, runAcceptanceChecks };
