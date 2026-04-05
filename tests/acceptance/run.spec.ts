import { expect, test } from "@playwright/test";
import {
  completeCsvRun,
  downloadRunArtifact,
  expectRunHiddenFromScope,
  otherScope,
  primaryScope,
} from "./support";

test("user can upload input, start a run, download the results, and keep them scoped", async ({
  request,
}) => {
  const completedRun = await completeCsvRun(
    request,
    primaryScope,
    "acceptance-a.csv",
    "name,email\nalice,alice@example.com\n",
  );

  const outputArtifact = await downloadRunArtifact(
    request,
    primaryScope,
    completedRun.runId,
    "output",
  );
  const logArtifact = await downloadRunArtifact(
    request,
    primaryScope,
    completedRun.runId,
    "log",
  );

  expect(outputArtifact.filePath).toBe(completedRun.outputPath);
  expect(outputArtifact.bytes.byteLength).toBeGreaterThan(0);
  expect(logArtifact.bytes.byteLength).toBeGreaterThan(0);

  await expectRunHiddenFromScope(request, otherScope, completedRun.runId);
});
