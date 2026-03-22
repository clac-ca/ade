function normalizeBaseUrl(value) {
  const trimmed = value.trim();

  if (trimmed === "") {
    throw new Error("ADE_BASE_URL must not be empty.");
  }

  return trimmed.replace(/\/+$/, "");
}

function assertString(value, description) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`Expected ${description} to be a non-empty string.`);
  }
}

async function expectOk(url, description) {
  const response = await fetch(url);

  if (!response.ok) {
    throw new Error(
      `Expected ${description} at ${url} to return 200, received ${response.status}.`,
    );
  }

  return response;
}

async function readJson(url, description) {
  const response = await expectOk(url, description);
  return await response.json();
}

async function assertAppShell(baseUrl) {
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

async function assertHealth(baseUrl) {
  const payload = await readJson(`${baseUrl}/api/healthz`, "API health check");

  if (payload.service !== "ade-api" || payload.status !== "ok") {
    throw new Error(
      'Expected /api/healthz to report service "ade-api" with status "ok".',
    );
  }
}

async function assertReady(baseUrl) {
  const payload = await readJson(
    `${baseUrl}/api/readyz`,
    "API readiness check",
  );

  if (payload.service !== "ade-api" || payload.status !== "ready") {
    throw new Error(
      'Expected /api/readyz to report service "ade-api" with status "ready".',
    );
  }
}

async function assertVersion(baseUrl) {
  const payload = await readJson(
    `${baseUrl}/api/version`,
    "API version metadata",
  );

  assertString(payload.service, "version payload service");
  assertString(payload.version, "version payload version");
  assertString(payload.gitSha, "version payload gitSha");
  assertString(payload.builtAt, "version payload builtAt");
  assertString(payload.nodeVersion, "version payload nodeVersion");

  if (payload.service !== "ade-api") {
    throw new Error('Expected /api/version to report service "ade-api".');
  }
}

async function assertApiRoot(baseUrl) {
  const payload = await readJson(`${baseUrl}/api/`, "API root endpoint");

  if (payload.service !== "ade-api" || payload.status !== "ok") {
    throw new Error(
      'Expected /api/ to report service "ade-api" with status "ok".',
    );
  }

  assertString(payload.version, "API root version");
}

async function runAcceptanceChecks(baseUrl) {
  const normalizedBaseUrl = normalizeBaseUrl(baseUrl);
  await assertAppShell(normalizedBaseUrl);
  await assertHealth(normalizedBaseUrl);
  await assertReady(normalizedBaseUrl);
  await assertVersion(normalizedBaseUrl);
  await assertApiRoot(normalizedBaseUrl);
}

export { runAcceptanceChecks };
