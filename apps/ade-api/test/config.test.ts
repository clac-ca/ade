import { mkdtempSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import * as assert from "node:assert";
import { test } from "node:test";
import { readConfig, readServerConfig } from "../src/config";

test("readConfig returns development build info when bundled build info is absent", () => {
  const config = readConfig(
    {},
    {
      buildInfoPath: join(tmpdir(), "missing-build-info.json"),
    },
  );

  assert.equal(config.buildInfo.service, "ade");
  assert.match(config.buildInfo.version, /\S+/);
  assert.match(config.buildInfo.gitSha, /\S+/);
  assert.match(config.buildInfo.builtAt, /\S+/);
  assert.equal(config.sqlConnectionString, undefined);
});

test("readConfig rejects missing SQL when required", () => {
  assert.throws(
    () =>
      readConfig(
        {},
        {
          buildInfoPath: join(tmpdir(), "missing-build-info.json"),
          requireSql: true,
        },
      ),
    /AZURE_SQL_CONNECTIONSTRING/,
  );
});

test("readConfig reads optional SQL settings", () => {
  const config = readConfig(
    {
      AZURE_SQL_CONNECTIONSTRING:
        "Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true",
    },
    {
      buildInfoPath: join(tmpdir(), "missing-build-info.json"),
    },
  );

  assert.match(config.sqlConnectionString ?? "", /Database=ade/);
});

test("readConfig rejects missing bundled build info in production", () => {
  assert.throws(
    () =>
      readConfig(
        {
          NODE_ENV: "production",
        },
        {
          buildInfoPath: join(tmpdir(), "missing-build-info.json"),
        },
      ),
    /Missing ADE build info/,
  );
});

test("readConfig rejects invalid bundled build info", () => {
  const tempDir = mkdtempSync(join(tmpdir(), "ade-build-info-invalid-"));
  const buildInfoPath = join(tempDir, "build-info.json");

  writeFileSync(
    buildInfoPath,
    JSON.stringify({
      builtAt: "",
      gitSha: "sha",
      service: "ade",
      version: "0.1.0",
    }),
  );

  assert.throws(
    () =>
      readConfig(
        {},
        {
          buildInfoPath,
        },
      ),
    /non-empty string/,
  );
});

test("readServerConfig applies defaults and accepts flags", () => {
  assert.deepStrictEqual(readServerConfig([], {}), {
    host: "127.0.0.1",
    port: 8000,
  });
  assert.deepStrictEqual(
    readServerConfig(["--host", "0.0.0.0", "--port", "9000"]),
    {
      host: "0.0.0.0",
      port: 9000,
    },
  );
});

test("readServerConfig rejects invalid ports", () => {
  assert.throws(
    () => readServerConfig(["--port", "70000"]),
    /--port/,
  );
});
