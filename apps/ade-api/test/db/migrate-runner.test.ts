import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  runMigrations,
  splitSqlBatches,
  type MigrationPoolLike,
} from "../../src/db/migrate-runner";
import { ConfigError, DatabaseError } from "../../src/errors";

test("splitSqlBatches returns a single batch when no GO separators are present", () => {
  assert.deepEqual(splitSqlBatches("SELECT 1;"), ["SELECT 1;"]);
});

test("splitSqlBatches splits on standalone GO separators", () => {
  assert.deepEqual(
    splitSqlBatches(`
      CREATE TABLE dbo.example (id INT NOT NULL);
      GO

      INSERT INTO dbo.example (id) VALUES (1);
      go
    `),
    [
      "CREATE TABLE dbo.example (id INT NOT NULL);",
      "INSERT INTO dbo.example (id) VALUES (1);",
    ],
  );
});

test("splitSqlBatches does not split on GO inside other tokens", () => {
  assert.deepEqual(
    splitSqlBatches(`
      SELECT 'go';
      SELECT N'Gopher';
    `),
    ["SELECT 'go';\n      SELECT N'Gopher';"],
  );
});

function createMigrationsDir(files: Record<string, string>): string {
  const migrationsDir = mkdtempSync(join(tmpdir(), "ade-migrations-"));

  for (const [name, sqlText] of Object.entries(files)) {
    writeFileSync(join(migrationsDir, name), sqlText);
  }

  return migrationsDir;
}

function createFakeMigrationPool(
  options: {
    appliedMigrations?: readonly string[];
    failOnBatchText?: string;
  } = {},
) {
  const adminBatches: string[] = [];
  const insertedMigrationNames: string[] = [];
  const migrationBatches: string[] = [];
  const migrationQueries: string[] = [];
  let beginCount = 0;
  let closeCount = 0;
  let commitCount = 0;
  let rollbackCount = 0;

  function createRootRequest() {
    return {
      async batch(statement: string) {
        adminBatches.push(statement);
      },
      input() {
        return this;
      },
      async query<T>(statement: string) {
        if (statement.includes(`FROM [dbo].[schema_migrations]`)) {
          return {
            recordset: (options.appliedMigrations ?? []).map(
              (migrationName) => ({
                migration_name: migrationName,
              }),
            ) as unknown as readonly T[],
          };
        }

        return {
          recordset: [] as readonly T[],
        };
      },
    };
  }

  function createTransactionRequest() {
    let migrationName: string | null = null;

    return {
      async batch(statement: string) {
        migrationBatches.push(statement);

        if (
          options.failOnBatchText &&
          statement.includes(options.failOnBatchText)
        ) {
          throw new Error(`failed batch: ${options.failOnBatchText}`);
        }
      },
      input(name: string, _type: unknown, value: unknown) {
        if (name === "migrationName" && typeof value === "string") {
          migrationName = value;
        }

        return this;
      },
      async query<T>(statement: string) {
        migrationQueries.push(statement);

        if (
          migrationName &&
          statement.includes(`INSERT INTO [dbo].[schema_migrations]`)
        ) {
          insertedMigrationNames.push(migrationName);
        }

        return {
          recordset: [] as readonly T[],
        };
      },
    };
  }

  const pool: MigrationPoolLike = {
    async close() {
      closeCount += 1;
    },
    request() {
      return createRootRequest();
    },
    transaction() {
      return {
        async begin() {
          beginCount += 1;
        },
        async commit() {
          commitCount += 1;
        },
        request() {
          return createTransactionRequest();
        },
        async rollback() {
          rollbackCount += 1;
        },
      };
    },
  };

  return {
    adminBatches,
    beginCount: () => beginCount,
    closeCount: () => closeCount,
    commitCount: () => commitCount,
    insertedMigrationNames,
    migrationBatches,
    migrationQueries,
    pool,
    rollbackCount: () => rollbackCount,
  };
}

test("runMigrations applies pending files and skips existing ones", async () => {
  const migrationsDir = createMigrationsDir({
    "001_create_schema.sql": "SELECT 1 AS created;",
    "002_seed_data.sql": "SELECT 2 AS seeded;",
  });
  const fake = createFakeMigrationPool({
    appliedMigrations: ["001_create_schema.sql"],
  });
  const ensuredConnections: string[] = [];

  const result = await runMigrations(
    {
      connectionString: "unused-connection",
      migrationsDir,
    },
    {
      connect: async () => fake.pool,
      ensureDatabaseExists: async (connectionString) => {
        ensuredConnections.push(connectionString);
      },
    },
  );

  assert.deepEqual(ensuredConnections, ["unused-connection"]);
  assert.deepEqual(result, {
    applied: ["002_seed_data.sql"],
    skipped: ["001_create_schema.sql"],
  });
  assert.equal(fake.beginCount(), 1);
  assert.equal(fake.commitCount(), 1);
  assert.equal(fake.rollbackCount(), 0);
  assert.equal(fake.closeCount(), 1);
  assert.deepEqual(fake.insertedMigrationNames, ["002_seed_data.sql"]);
  assert.ok(
    fake.adminBatches.some((statement) =>
      statement.includes("CREATE TABLE dbo.schema_migrations"),
    ),
  );
  assert.ok(fake.migrationBatches.includes("SELECT 2 AS seeded;"));
});

test("runMigrations rejects empty migration files", async () => {
  const migrationsDir = createMigrationsDir({
    "001_empty.sql": "   \n\t",
  });
  const fake = createFakeMigrationPool();

  await assert.rejects(
    () =>
      runMigrations(
        {
          connectionString: "unused-connection",
          migrationsDir,
        },
        {
          connect: async () => fake.pool,
          ensureDatabaseExists: async () => undefined,
        },
      ),
    (error) =>
      error instanceof ConfigError &&
      error.message.includes("Migration file is empty"),
  );

  assert.equal(fake.beginCount(), 0);
  assert.equal(fake.closeCount(), 1);
});

test("runMigrations rolls back the failing migration and stops processing later files", async () => {
  const migrationsDir = createMigrationsDir({
    "001_first.sql": "SELECT 1 AS first;",
    "002_second.sql": "SELECT 2 AS second;\nGO\nSELECT 3 AS boom;",
    "003_third.sql": "SELECT 4 AS third;",
  });
  const fake = createFakeMigrationPool({
    failOnBatchText: "SELECT 3 AS boom;",
  });

  await assert.rejects(
    () =>
      runMigrations(
        {
          connectionString: "unused-connection",
          migrationsDir,
        },
        {
          connect: async () => fake.pool,
          ensureDatabaseExists: async () => undefined,
        },
      ),
    (error) =>
      error instanceof DatabaseError &&
      error.message === "Failed to apply SQL migration: 002_second.sql",
  );

  assert.equal(fake.beginCount(), 2);
  assert.equal(fake.commitCount(), 1);
  assert.equal(fake.rollbackCount(), 1);
  assert.equal(fake.closeCount(), 1);
  assert.deepEqual(fake.insertedMigrationNames, ["001_first.sql"]);
  assert.ok(fake.migrationBatches.includes("SELECT 1 AS first;"));
  assert.ok(fake.migrationBatches.includes("SELECT 2 AS second;"));
  assert.ok(fake.migrationBatches.includes("SELECT 3 AS boom;"));
  assert.ok(!fake.migrationBatches.includes("SELECT 4 AS third;"));
});
