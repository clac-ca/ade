import test from "node:test";
import assert from "node:assert/strict";
import {
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localComposeProjectName,
  localSqlPassword,
  localSqlPort,
  localWebPort,
} from "../lib/dev-config";
import { readOptionalTrimmedString } from "../lib/runtime";

test("local development defaults are fixed and predictable", () => {
  assert.equal(localApiHost, "127.0.0.1");
  assert.equal(localApiPort, 8000);
  assert.equal(localComposeProjectName, "ade-local");
  assert.equal(localSqlPort, 8013);
  assert.equal(localSqlPassword, "AdeLocal1!adeclean");
  assert.equal(localWebPort, 5173);
  assert.equal(
    createLocalSqlConnectionString(),
    "Server=127.0.0.1,8013;Database=ade;User Id=sa;Password=AdeLocal1!adeclean;Encrypt=false;TrustServerCertificate=true",
  );
});

test("readOptionalTrimmedString ignores missing and blank values", () => {
  assert.equal(readOptionalTrimmedString({}, "MISSING"), undefined);
  assert.equal(readOptionalTrimmedString({ VALUE: "   " }, "VALUE"), undefined);
  assert.equal(readOptionalTrimmedString({ VALUE: " ok " }, "VALUE"), "ok");
});
