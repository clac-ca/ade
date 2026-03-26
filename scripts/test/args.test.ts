import test from "node:test";
import assert from "node:assert/strict";
import { parseAcceptanceArgs, parseDevArgs, parseStartArgs } from "../lib/args";

test("parseDevArgs returns the default web port and closed-browser default", () => {
  assert.deepEqual(parseDevArgs([]), {
    noOpen: false,
    port: 5173,
  });
});

test("parseDevArgs accepts an explicit port and --no-open", () => {
  assert.deepEqual(parseDevArgs(["--port", "8100", "--no-open"]), {
    noOpen: true,
    port: 8100,
  });
});

test("parseStartArgs accepts an image override", () => {
  assert.deepEqual(
    parseStartArgs(["--port", "9000", "--image", "ghcr.io/example/ade:test"]),
    {
      image: "ghcr.io/example/ade:test",
      noOpen: false,
      port: 9000,
    },
  );
});

test("parseAcceptanceArgs requires a valid url", () => {
  assert.equal(
    parseAcceptanceArgs([
      "--url",
      "https://ade.example.com/base/",
    ]).url.toString(),
    "https://ade.example.com/base/",
  );
  assert.throws(() => parseAcceptanceArgs([]), /Missing required --url/);
  assert.throws(
    () => parseAcceptanceArgs(["--url", "not-a-url"]),
    /Invalid --url/,
  );
});
