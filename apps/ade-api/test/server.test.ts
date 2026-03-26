import * as assert from "node:assert";
import { test } from "node:test";
import { runServer } from "../src/server";

function createLogger() {
  const errors: string[] = [];

  return {
    errors,
    logger: {
      error(message: string) {
        errors.push(message);
      },
      info() {},
    },
  };
}

test("runServer exits non-zero when shutdown fails", async () => {
  let signalHandler: (() => void) | undefined;
  const exitCodes: number[] = [];
  const { errors, logger } = createLogger();
  const processHandle = {
    exit(code: number) {
      exitCodes.push(code);
    },
    on(event: string, handler: () => void) {
      if (event === "SIGTERM") {
        signalHandler = handler;
      }
    },
  };

  const runtime = {
    start: async () => {},
    stop: async () => {
      throw new Error("close failed");
    },
  };

  await runServer(processHandle, runtime, logger);
  const stopServer = signalHandler;
  assert.ok(stopServer);

  stopServer();
  await new Promise((resolve) => {
    setImmediate(resolve);
  });
  assert.deepStrictEqual(exitCodes, [1]);
  assert.equal(errors.length, 1);
});

test("runServer exits non-zero when startup fails", async () => {
  const exitCodes: number[] = [];
  const { errors, logger } = createLogger();
  const processHandle = {
    exit(code: number) {
      exitCodes.push(code);
    },
    on() {},
  };

  const runtime = {
    start: async () => {
      throw new Error("sql unavailable");
    },
    stop: async () => {},
  };

  await runServer(processHandle, runtime, logger);
  assert.deepStrictEqual(exitCodes, [1]);
  assert.equal(errors.length, 1);
});
