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

test("parseAcceptanceArgs defaults to a managed local environment", () => {
  assert.deepEqual(parseAcceptanceArgs([]), {
    image: "ade:local",
    mode: "managed",
    port: 4100,
  });
});

test("parseAcceptanceArgs requires a valid url in attach mode", () => {
  const config = parseAcceptanceArgs([
    "--url",
    "https://ade.example.com/base/",
  ]);

  assert.equal(config.mode, "attach");
  assert.equal(config.url.toString(), "https://ade.example.com/base/");
  assert.throws(
    () => parseAcceptanceArgs(["--url", "not-a-url"]),
    /Invalid --url/,
  );
  assert.throws(
    () =>
      parseAcceptanceArgs([
        "--url",
        "https://ade.example.com",
        "--port",
        "4101",
      ]),
    /cannot be combined/,
  );
});
