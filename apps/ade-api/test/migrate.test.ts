import * as assert from "node:assert";
import { test } from "node:test";
import { main } from "../src/migrate";

test("main reads migration config and reports applied and skipped migrations", async () => {
  const messages: string[] = [];
  let options: { connectionString: string } | undefined;

  await main(
    {
      error(message: string) {
        messages.push(`error:${message}`);
      },
      info(message: string) {
        messages.push(message);
      },
    },
    {
      env: {
        AZURE_SQL_CONNECTIONSTRING: "Server=sql;Database=ade;",
      },
      run: async (value) => {
        options = value;

        return {
          applied: ["001_create_schema.sql"],
          skipped: ["000_bootstrap.sql"],
        };
      },
    },
  );

  assert.deepStrictEqual(options, {
    connectionString: "Server=sql;Database=ade;",
  });
  assert.deepStrictEqual(messages, [
    "Applied migration: 001_create_schema.sql",
    "Skipped migration: 000_bootstrap.sql",
    "Migration complete. Applied 1, skipped 1.",
  ]);
});
