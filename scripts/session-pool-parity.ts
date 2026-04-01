import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { createLocalSessionPoolManagementEndpoint } from "./lib/dev-config";
import { runMain } from "./lib/runtime";

const localBearerToken = "ade-local-session-token";
const baselinePath = fileURLToPath(
  new URL("../infra/local/sessionpool/azure-shell-baseline.json", import.meta.url),
);

type Baseline = {
  apiVersion: string;
  supportedVersionsHeader: string;
  execution: {
    failureStatus: string;
    successStatus: string;
    timeoutStatus: string;
  };
  filesystem: {
    pathScopedUploadRoot: string;
    rootUploadRoot: string;
  };
  host: {
    arch: string;
    cwd: string;
    glibc: string;
    home: string;
    path: string;
    shell: string;
    user: string;
  };
  metadata: unknown;
};

type ExecutionResponse = {
  identifier: string;
  result: {
    executionTimeInMilliseconds: number;
    stderr: string;
    stdout: string;
  };
  status: string;
};

async function deleteSession(baseUrl: string, apiVersion: string, identifier: string) {
  await fetch(`${baseUrl}/session?api-version=${apiVersion}&identifier=${identifier}`, {
    headers: authorizationHeaders(),
    method: "DELETE",
  });
}

async function expectJson<T>(response: Response): Promise<T> {
  const text = await response.text();
  try {
    return JSON.parse(text) as T;
  } catch (error) {
    throw new Error(
      `Expected JSON response but received: ${text}\n${
        error instanceof Error ? error.message : String(error)
      }`,
      { cause: error },
    );
  }
}

async function getJson(url: string): Promise<{ headers: Headers; value: unknown }> {
  const response = await fetch(url, {
    headers: authorizationHeaders(),
  });
  assert.equal(response.status, 200, `GET ${url} should succeed`);
  return {
    headers: response.headers,
    value: await expectJson(response),
  };
}

function authorizationHeaders(): Record<string, string> {
  return {
    Authorization: `Bearer ${localBearerToken}`,
  };
}

async function postExecution(
  baseUrl: string,
  apiVersion: string,
  identifier: string,
  body: Record<string, unknown>,
): Promise<{ headers: Headers; value: ExecutionResponse }> {
  const response = await fetch(
    `${baseUrl}/executions?api-version=${apiVersion}&identifier=${identifier}`,
    {
      body: JSON.stringify(body),
      headers: {
        ...authorizationHeaders(),
        "Content-Type": "application/json",
      },
      method: "POST",
    },
  );
  assert.equal(response.status, 200, "execution should succeed");
  return {
    headers: response.headers,
    value: await expectJson<ExecutionResponse>(response),
  };
}

async function uploadFile(
  baseUrl: string,
  apiVersion: string,
  identifier: string,
  filename: string,
  contents: string,
  path?: string,
) {
  const form = new FormData();
  form.append(
    "file",
    new Blob([contents], { type: "text/plain" }),
    filename,
  );
  const query = new URLSearchParams({
    "api-version": apiVersion,
    identifier,
  });
  if (path) {
    query.set("path", path);
  }

  const response = await fetch(`${baseUrl}/files?${query.toString()}`, {
    body: form,
    headers: authorizationHeaders(),
    method: "POST",
  });
  assert.equal(response.status, 200, "file upload should succeed");
  return {
    headers: response.headers,
    value: await expectJson<{
      contentType: string;
      lastModifiedAt?: string;
      name: string;
      sizeInBytes: number;
      type: string;
    }>(response),
  };
}

async function main() {
  const baseline = JSON.parse(readFileSync(baselinePath, "utf8")) as Baseline;
  const baseUrl =
    process.env["ADE_SESSION_POOL_MANAGEMENT_ENDPOINT"] ??
    createLocalSessionPoolManagementEndpoint();
  const identifier = `parity-${String(Date.now())}`;

  try {
    const metadata = await getJson(
      `${baseUrl}/metadata?api-version=${baseline.apiVersion}`,
    );
    assert.deepEqual(metadata.value, baseline.metadata);
    assert.equal(
      metadata.headers.get("api-supported-versions"),
      baseline.supportedVersionsHeader,
    );

    const hostInfo = await postExecution(baseUrl, baseline.apiVersion, identifier, {
      shellCommand:
        'printf "{\\"arch\\":\\"%s\\",\\"cwd\\":\\"%s\\",\\"glibc\\":\\"%s\\",\\"home\\":\\"%s\\",\\"os\\":\\"%s\\",\\"path\\":\\"%s\\",\\"shell\\":\\"%s\\",\\"user\\":\\"%s\\",\\"version\\":\\"%s\\"}\\n" "$(uname -m)" "$PWD" "$(ldd --version 2>&1 | head -n1 | sed -E \'s/.* ([0-9]+\\.[0-9]+)$/\\1/\')" "$HOME" "$(. /etc/os-release; printf %s "$NAME")" "$PATH" "$SHELL" "$(id -un)" "$(. /etc/os-release; printf %s "$VERSION")"',
    });
    assert.equal(hostInfo.value.status, baseline.execution.successStatus);
    assert.deepEqual(
      JSON.parse(hostInfo.value.result.stdout),
      baseline.host,
      "host surface should match the Azure Shell baseline",
    );

    const rootUpload = await uploadFile(
      baseUrl,
      baseline.apiVersion,
      identifier,
      "root-file.txt",
      "root-file",
    );
    assert.equal(rootUpload.value.contentType, "text/plain; charset=utf-8");
    assert.equal(rootUpload.value.lastModifiedAt !== undefined, true);
    assert.equal(rootUpload.value.name, "root-file.txt");
    assert.equal(rootUpload.value.sizeInBytes, 9);
    assert.equal(rootUpload.value.type, "file");

    const pathUpload = await uploadFile(
      baseUrl,
      baseline.apiVersion,
      identifier,
      "path-file.txt",
      "path-file",
      "ade/bin",
    );
    assert.equal(pathUpload.value.contentType, "text/plain; charset=utf-8");
    assert.equal(pathUpload.value.lastModifiedAt !== undefined, true);
    assert.equal(pathUpload.value.name, "path-file.txt");
    assert.equal(pathUpload.value.sizeInBytes, 9);
    assert.equal(pathUpload.value.type, "file");

    const rootListing = await getJson(
      `${baseUrl}/files?api-version=${baseline.apiVersion}&identifier=${identifier}`,
    );
    assert.deepEqual(
      (rootListing.value as { value: Array<{ name: string }> }).value.map(
        (entry) => entry.name,
      ),
      ["root-file.txt"],
    );

    const pathListing = await getJson(
      `${baseUrl}/files?api-version=${baseline.apiVersion}&identifier=${identifier}&path=ade/bin`,
    );
    assert.deepEqual(
      (pathListing.value as { value: Array<{ name: string }> }).value.map(
        (entry) => entry.name,
      ),
      ["path-file.txt"],
    );

    const locations = await postExecution(baseUrl, baseline.apiVersion, identifier, {
      shellCommand:
        'printf "{\\"root\\":%s,\\"pathScoped\\":%s,\\"rootPath\\":%s,\\"pathPath\\":%s}\\n" "$(test -f /mnt/data/root-file.txt && echo true || echo false)" "$(test -f /app/ade/bin/path-file.txt && echo true || echo false)" "$(test -f /app/root-file.txt && echo true || echo false)" "$(test -f /mnt/data/ade/bin/path-file.txt && echo true || echo false)"',
    });
    assert.equal(locations.value.status, baseline.execution.successStatus);
    assert.deepEqual(JSON.parse(locations.value.result.stdout), {
      pathPath: false,
      pathScoped: true,
      root: true,
      rootPath: false,
    });

    const timeout = await postExecution(baseUrl, baseline.apiVersion, identifier, {
      shellCommand: "sleep 2",
      timeoutInSeconds: 1,
    });
    assert.equal(timeout.value.status, baseline.execution.timeoutStatus);
    assert.equal(timeout.value.result.executionTimeInMilliseconds, 1000);

    const invalidPath = await fetch(
      `${baseUrl}/files?api-version=${baseline.apiVersion}&identifier=${identifier}&path=.ade/bin`,
      {
        body: (() => {
          const form = new FormData();
          form.append(
            "file",
            new Blob(["bad"], { type: "text/plain" }),
            "bad.txt",
          );
          return form;
        })(),
        headers: authorizationHeaders(),
        method: "POST",
      },
    );
    assert.equal(invalidPath.status, 400);
    const invalidPathBody = await expectJson<{
      error: { code: string; message: string; traceId: string };
    }>(invalidPath);
    assert.equal(invalidPathBody.error.code, "FilePathInvalid");
    assert.equal(
      invalidPathBody.error.message,
      "File Path '/.ade/bin' is invalid because 'path cannot contain any reserved file path characters'.",
    );
    assert.ok(invalidPathBody.error.traceId.length > 0);

    console.log(
      JSON.stringify(
        {
          baseUrl,
          identifier,
          result: "ok",
        },
        null,
        2,
      ),
    );
  } finally {
    await deleteSession(baseUrl, baseline.apiVersion, identifier);
  }
}

void runMain(main);
