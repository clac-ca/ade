import test from "node:test";
import assert from "node:assert/strict";
import { waitForReady } from "../lib/shell";

test("waitForReady returns when every URL responds successfully", async (t) => {
  const originalFetch = globalThis.fetch;
  let callCount = 0;

  globalThis.fetch = (async () => {
    callCount += 1;
    return new Response(null, {
      status: 200,
    });
  }) as typeof fetch;

  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  await waitForReady(
    ["http://example.test/", "http://example.test/api/readyz"],
    {
      timeoutMs: 500,
    },
  );

  assert.equal(callCount, 2);
});

test("waitForReady fails fast when a required process exits", async (t) => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async () => {
    throw new Error("not ready");
  }) as typeof fetch;

  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  await assert.rejects(
    () =>
      waitForReady(["http://example.test/api/readyz"], {
        isAlive: () => false,
        timeoutMs: 500,
      }),
    /required process exited/,
  );
});
