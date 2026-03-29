import test from "node:test";
import assert from "node:assert/strict";
import {
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalSessionPoolManagementEndpoint,
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localContainerAppUrl,
  localComposeProjectName,
  localSessionPoolPort,
  localSessionPoolSecret,
  localSqlPassword,
  localSqlPort,
  localWebHost,
  localWebPort,
} from "../lib/dev-config";
import { readOptionalTrimmedString } from "../lib/runtime";

test("local development defaults are fixed and predictable", () => {
  assert.equal(localApiHost, "127.0.0.1");
  assert.equal(localApiPort, 8000);
  assert.equal(localComposeProjectName, "ade-local");
  assert.equal(localContainerAppUrl, "http://host.docker.internal:5173");
  assert.equal(localSessionPoolPort, 8014);
  assert.equal(localSessionPoolSecret, "ade-local-session-secret");
  assert.equal(localSqlPort, 8013);
  assert.equal(localSqlPassword, "AdeLocal1!adeclean");
  assert.equal(localWebHost, "0.0.0.0");
  assert.equal(localWebPort, 5173);
  assert.equal(
    createLocalSqlConnectionString(),
    "Server=127.0.0.1,8013;Database=ade;User Id=sa;Password=AdeLocal1!adeclean;Encrypt=false;TrustServerCertificate=true",
  );
  assert.equal(
    createLocalSessionPoolManagementEndpoint(),
    "http://127.0.0.1:8014",
  );
  assert.equal(
    createLocalContainerSessionPoolManagementEndpoint(),
    "http://host.docker.internal:8014",
  );
});

test("readOptionalTrimmedString ignores missing and blank values", () => {
  assert.equal(readOptionalTrimmedString({}, "MISSING"), undefined);
  assert.equal(readOptionalTrimmedString({ VALUE: "   " }, "VALUE"), undefined);
  assert.equal(readOptionalTrimmedString({ VALUE: " ok " }, "VALUE"), "ok");
});
