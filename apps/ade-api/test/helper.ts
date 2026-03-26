import { join } from "node:path";
import * as test from "node:test";
import { createApp } from "../src/app";
import type { BundledBuildInfo } from "../src/config";
import {
  createReadinessController,
  type ReadinessPhase,
} from "../src/readiness";

export type TestContext = {
  after: typeof test.after;
};

export type BuildOptions = {
  buildInfo?: BundledBuildInfo;
  databaseError?: string | null;
  databaseOk?: boolean;
  lastCheckedAt?: number | null;
  phase?: ReadinessPhase;
  staleAfterMs?: number;
};

const defaultBuildInfo: BundledBuildInfo = {
  service: "ade",
  version: "test-version",
  gitSha: "test-git-sha",
  builtAt: "2026-03-21T00:00:00.000Z",
};
const webRoot = join(__dirname, "fixtures", "web-dist");

async function build(t: TestContext, options: BuildOptions = {}) {
  const readiness = createReadinessController({
    databaseOk: options.databaseOk ?? true,
    lastCheckedAt: options.lastCheckedAt ?? Date.now(),
    lastError: options.databaseError ?? null,
    phase: options.phase ?? "ready",
    ...(options.staleAfterMs !== undefined
      ? {
          staleAfterMs: options.staleAfterMs,
        }
      : {}),
  });

  const fastify = createApp({
    buildInfo: options.buildInfo ?? defaultBuildInfo,
    getReadinessSnapshot: () => readiness.snapshot(),
    logger: false,
    webRoot,
  });

  await fastify.ready();

  t.after(() => void fastify.close());
  return {
    app: fastify,
    buildInfo: options.buildInfo ?? defaultBuildInfo,
    readiness,
  };
}

export { build };
