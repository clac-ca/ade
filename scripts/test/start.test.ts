import test from "node:test";
import assert from "node:assert/strict";
import { createContainerRunArgs } from "../lib/start";

test("createContainerRunArgs builds a predictable docker run command", () => {
  assert.deepEqual(
    createContainerRunArgs({
      containerName: "ade-local-8000",
      hostPort: 8000,
      image: "ade:test",
    }),
    [
      "run",
      "--rm",
      "--name",
      "ade-local-8000",
      "--add-host",
      "host.docker.internal:host-gateway",
      "--publish",
      "8000:8000",
      "--env",
      "AZURE_SQL_CONNECTIONSTRING",
      "ade:test",
    ],
  );
});
