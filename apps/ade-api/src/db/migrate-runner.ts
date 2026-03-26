import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import sql from "mssql";
import { connectToDatabase, ensureDatabaseExists } from "./connection";
import { AppError, ConfigError, DatabaseError } from "../errors";

export type MigrationResult = {
  applied: string[];
  skipped: string[];
};

export type RunMigrationsOptions = {
  connectionString: string;
  migrationsDir?: string;
};

type MigrationQueryResultLike<T> = {
  recordset: readonly T[];
};

type MigrationRequestLike = {
  batch(statement: string): Promise<unknown>;
  input(name: string, type: unknown, value: unknown): MigrationRequestLike;
  query<T>(statement: string): Promise<MigrationQueryResultLike<T>>;
};

type MigrationTransactionLike = {
  begin(): Promise<unknown>;
  commit(): Promise<void>;
  request(): MigrationRequestLike;
  rollback(): Promise<void>;
};

type MigrationPoolLike = {
  close(): Promise<void>;
  request(): MigrationRequestLike;
  transaction(): MigrationTransactionLike;
};

type RunMigrationsDependencies = {
  connect?: (connectionString: string) => Promise<MigrationPoolLike>;
  ensureDatabaseExists?: (connectionString: string) => Promise<void>;
  readMigrationText?: (migrationPath: string) => string;
};

const schemaMigrationsTable = "[dbo].[schema_migrations]";

function throwMigrationError(message: string, error: unknown): never {
  if (error instanceof AppError) {
    throw error;
  }

  throw new DatabaseError(message, error);
}

function readMigrationFiles(migrationsDir: string): string[] {
  if (!existsSync(migrationsDir)) {
    return [];
  }

  return readdirSync(migrationsDir)
    .filter((entry) => entry.endsWith(".sql"))
    .sort((left, right) => left.localeCompare(right));
}

function splitSqlBatches(sqlText: string): string[] {
  return sqlText
    .split(/^\s*GO\s*$/gim)
    .map((batch) => batch.trim())
    .filter((batch) => batch !== "");
}

async function ensureSchemaMigrationsTable(pool: MigrationPoolLike) {
  try {
    await pool.request().batch(`
      IF OBJECT_ID(N'dbo.schema_migrations', N'U') IS NULL
      BEGIN
        CREATE TABLE dbo.schema_migrations (
          migration_name NVARCHAR(255) NOT NULL PRIMARY KEY,
          applied_at_utc DATETIME2(7) NOT NULL CONSTRAINT DF_schema_migrations_applied_at_utc DEFAULT SYSUTCDATETIME()
        );
      END
    `);
  } catch (error) {
    throwMigrationError("Failed to ensure the schema migrations table.", error);
  }
}

async function readAppliedMigrations(
  pool: MigrationPoolLike,
): Promise<Set<string>> {
  let result: MigrationQueryResultLike<{ migration_name: string }>;

  try {
    result = await pool.request().query<{ migration_name: string }>(`
      SELECT migration_name
      FROM ${schemaMigrationsTable}
    `);
  } catch (error) {
    throwMigrationError("Failed to read applied SQL migrations.", error);
  }

  return new Set(result.recordset.map((row) => row.migration_name));
}

async function applyMigration(
  pool: MigrationPoolLike,
  migrationName: string,
  sqlText: string,
) {
  const transaction = pool.transaction();

  try {
    await transaction.begin();

    for (const batch of splitSqlBatches(sqlText)) {
      await transaction.request().batch(batch);
    }

    await transaction
      .request()
      .input("migrationName", sql.NVarChar(255), migrationName).query(`
        INSERT INTO ${schemaMigrationsTable} (migration_name)
        VALUES (@migrationName)
      `);

    await transaction.commit();
  } catch (error) {
    await transaction.rollback().catch(() => undefined);
    throwMigrationError(
      `Failed to apply SQL migration: ${migrationName}`,
      error,
    );
  }
}

async function runMigrations(
  {
    connectionString,
    migrationsDir = join(process.cwd(), "migrations"),
  }: RunMigrationsOptions,
  dependencies: RunMigrationsDependencies = {},
): Promise<MigrationResult> {
  const ensureDatabase =
    dependencies.ensureDatabaseExists ?? ensureDatabaseExists;
  const connect = dependencies.connect ?? connectToDatabase;
  const readMigrationText =
    dependencies.readMigrationText ??
    ((migrationPath: string) => readFileSync(migrationPath, "utf8"));

  await ensureDatabase(connectionString);

  let pool: MigrationPoolLike;

  try {
    pool = await connect(connectionString);
  } catch (error) {
    throwMigrationError("Failed to connect to SQL for migrations.", error);
  }

  try {
    await ensureSchemaMigrationsTable(pool);

    const applied = await readAppliedMigrations(pool);
    const migrationFiles = readMigrationFiles(migrationsDir);
    const result: MigrationResult = {
      applied: [],
      skipped: [],
    };

    for (const migrationName of migrationFiles) {
      if (applied.has(migrationName)) {
        result.skipped.push(migrationName);
        continue;
      }

      const sqlText = readMigrationText(
        join(migrationsDir, migrationName),
      ).trim();

      if (sqlText === "") {
        throw new ConfigError(`Migration file is empty: ${migrationName}`);
      }

      await applyMigration(pool, migrationName, sqlText);
      result.applied.push(migrationName);
    }

    return result;
  } finally {
    await pool.close();
  }
}

export { readMigrationFiles, runMigrations, splitSqlBatches };

export type {
  MigrationPoolLike,
  MigrationRequestLike,
  MigrationTransactionLike,
  RunMigrationsDependencies,
};
