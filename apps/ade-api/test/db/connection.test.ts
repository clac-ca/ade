import * as assert from "node:assert";
import { test } from "node:test";
import {
  buildPoolConfig,
  ensureDatabaseExists,
  parseSqlConnectionString,
  quoteSqlIdentifier,
  withDatabase,
} from "../../src/db/connection";

test("parseSqlConnectionString supports SQL auth connections", () => {
  const parsed = parseSqlConnectionString(
    "Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true",
  );

  assert.deepStrictEqual(parsed, {
    authentication: "sql-password",
    database: "ade",
    encrypt: false,
    password: "Password!234",
    port: 1433,
    server: "127.0.0.1",
    trustServerCertificate: true,
    userId: "sa",
  });
});

test("parseSqlConnectionString supports quoted semicolons in SQL auth passwords", () => {
  const parsed = parseSqlConnectionString(
    'Server=127.0.0.1,1433;Database=ade;User Id=sa;Password="Password;!;234";Encrypt=false;TrustServerCertificate=true',
  );

  assert.equal(parsed.password, "Password;!;234");
});

test("parseSqlConnectionString supports managed identity connections", () => {
  const parsed = parseSqlConnectionString(
    "Data Source=tcp:sql-ade.database.windows.net,1433;Initial Catalog=ade;User ID=11111111-1111-1111-1111-111111111111;Authentication=Active Directory Managed Identity;Encrypt=true;TrustServerCertificate=false",
  );

  assert.deepStrictEqual(parsed, {
    authentication: "managed-identity",
    database: "ade",
    encrypt: true,
    port: 1433,
    server: "sql-ade.database.windows.net",
    trustServerCertificate: false,
    userId: "11111111-1111-1111-1111-111111111111",
  });
});

test("buildPoolConfig translates managed identity connections into mssql config", () => {
  const config = buildPoolConfig(
    "Data Source=tcp:sql-ade.database.windows.net,1433;Initial Catalog=ade;Authentication=Active Directory Default;Encrypt=true;TrustServerCertificate=false",
  );

  assert.equal(config.authentication?.type, "azure-active-directory-default");
  assert.equal(config.database, "ade");
  assert.equal(config.port, 1433);
  assert.equal(config.server, "sql-ade.database.windows.net");
});

test("buildPoolConfig forwards managed identity client IDs", () => {
  const config = buildPoolConfig(
    "Data Source=tcp:sql-ade.database.windows.net,1433;Initial Catalog=ade;User ID=11111111-1111-1111-1111-111111111111;Authentication=Active Directory Managed Identity;Encrypt=true;TrustServerCertificate=false",
  );

  assert.deepStrictEqual(config.authentication, {
    options: {
      clientId: "11111111-1111-1111-1111-111111111111",
    },
    type: "azure-active-directory-default",
  });
});

test("quoteSqlIdentifier escapes closing brackets", () => {
  assert.equal(quoteSqlIdentifier("ade]identity"), "[ade]]identity]");
});

test("withDatabase rewrites the database name", () => {
  assert.equal(
    withDatabase(
      "Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234",
      "master",
    ),
    "data source={127.0.0.1,1433};initial catalog=master;user id=sa;password={Password!234}",
  );
});

test("ensureDatabaseExists connects to master and creates the target database for SQL auth", async () => {
  const connections: string[] = [];
  const batches: string[] = [];
  let closed = false;

  await ensureDatabaseExists(
    "Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true",
    async (connectionString) => {
      connections.push(connectionString);

      return {
        close: async () => {
          closed = true;
        },
        request: () => ({
          batch: async (statement: string) => {
            batches.push(statement);
          },
        }),
      };
    },
  );

  assert.deepStrictEqual(connections, [
    "data source={127.0.0.1,1433};initial catalog=master;user id=sa;password={Password!234};encrypt=No;trustservercertificate=Yes",
  ]);
  assert.equal(closed, true);
  assert.equal(batches.length, 1);
  const batch = batches[0];
  assert.ok(batch);
  assert.match(batch, /DB_ID\(N'ade'\)/);
  assert.match(batch, /CREATE DATABASE \[ade\]/);
});

test("ensureDatabaseExists skips database creation for managed identity connections", async () => {
  let connected = false;

  await ensureDatabaseExists(
    "Data Source=tcp:sql-ade.database.windows.net,1433;Initial Catalog=ade;Authentication=ActiveDirectoryManagedIdentity;Encrypt=true;TrustServerCertificate=false",
    async () => {
      connected = true;

      return {
        close: async () => undefined,
        request: () => ({
          batch: async () => undefined,
        }),
      };
    },
  );

  assert.equal(connected, false);
});
