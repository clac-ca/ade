import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import sql from 'mssql'
import { connectToSql, quoteSqlIdentifier } from './connection'

export type MigrationResult = {
  applied: string[],
  skipped: string[]
}

export type RunMigrationsOptions = {
  connectionString: string,
  migrationsDir?: string,
  runtimePrincipalName?: string
}

const schemaMigrationsTable = '[dbo].[schema_migrations]'

function readMigrationFiles(migrationsDir: string): string[] {
  if (!existsSync(migrationsDir)) {
    return []
  }

  return readdirSync(migrationsDir)
    .filter((entry) => entry.endsWith('.sql'))
    .sort((left, right) => left.localeCompare(right))
}

function splitSqlBatches(sqlText: string): string[] {
  return sqlText
    .split(/^\s*GO\s*$/gim)
    .map((batch) => batch.trim())
    .filter((batch) => batch !== '')
}

async function ensureSchemaMigrationsTable(pool: sql.ConnectionPool) {
  await pool.request().batch(`
    IF OBJECT_ID(N'dbo.schema_migrations', N'U') IS NULL
    BEGIN
      CREATE TABLE dbo.schema_migrations (
        migration_name NVARCHAR(255) NOT NULL PRIMARY KEY,
        applied_at_utc DATETIME2(7) NOT NULL CONSTRAINT DF_schema_migrations_applied_at_utc DEFAULT SYSUTCDATETIME()
      );
    END
  `)
}

async function ensureRuntimePrincipal(pool: sql.ConnectionPool, runtimePrincipalName: string) {
  const principalIdentifier = quoteSqlIdentifier(runtimePrincipalName)

  await pool.request().batch(`
    IF NOT EXISTS (
      SELECT 1
      FROM sys.database_principals
      WHERE name = N'${runtimePrincipalName.replaceAll("'", "''")}'
    )
    BEGIN
      EXEC(N'CREATE USER ${principalIdentifier} FROM EXTERNAL PROVIDER');
    END

    IF NOT EXISTS (
      SELECT 1
      FROM sys.database_role_members AS role_members
      INNER JOIN sys.database_principals AS role_principals ON role_principals.principal_id = role_members.role_principal_id
      INNER JOIN sys.database_principals AS member_principals ON member_principals.principal_id = role_members.member_principal_id
      WHERE role_principals.name = N'db_datareader'
        AND member_principals.name = N'${runtimePrincipalName.replaceAll("'", "''")}'
    )
    BEGIN
      EXEC(N'ALTER ROLE [db_datareader] ADD MEMBER ${principalIdentifier}');
    END

    IF NOT EXISTS (
      SELECT 1
      FROM sys.database_role_members AS role_members
      INNER JOIN sys.database_principals AS role_principals ON role_principals.principal_id = role_members.role_principal_id
      INNER JOIN sys.database_principals AS member_principals ON member_principals.principal_id = role_members.member_principal_id
      WHERE role_principals.name = N'db_datawriter'
        AND member_principals.name = N'${runtimePrincipalName.replaceAll("'", "''")}'
    )
    BEGIN
      EXEC(N'ALTER ROLE [db_datawriter] ADD MEMBER ${principalIdentifier}');
    END
  `)
}

async function readAppliedMigrations(pool: sql.ConnectionPool): Promise<Set<string>> {
  const result = await pool.request().query<{ migration_name: string }>(`
    SELECT migration_name
    FROM ${schemaMigrationsTable}
  `)

  return new Set(result.recordset.map((row: { migration_name: string }) => row.migration_name))
}

async function applyMigration(pool: sql.ConnectionPool, migrationName: string, sqlText: string) {
  const transaction = new sql.Transaction(pool)
  await transaction.begin()

  try {
    for (const batch of splitSqlBatches(sqlText)) {
      await new sql.Request(transaction).batch(batch)
    }

    await new sql.Request(transaction)
      .input('migrationName', sql.NVarChar(255), migrationName)
      .query(`
        INSERT INTO ${schemaMigrationsTable} (migration_name)
        VALUES (@migrationName)
      `)

    await transaction.commit()
  } catch (error) {
    await transaction.rollback().catch(() => undefined)
    throw error
  }
}

async function runMigrations({
  connectionString,
  migrationsDir = join(process.cwd(), 'migrations'),
  runtimePrincipalName
}: RunMigrationsOptions): Promise<MigrationResult> {
  const pool = await connectToSql(connectionString)

  try {
    await ensureSchemaMigrationsTable(pool)

    if (runtimePrincipalName) {
      await ensureRuntimePrincipal(pool, runtimePrincipalName)
    }

    const applied = await readAppliedMigrations(pool)
    const migrationFiles = readMigrationFiles(migrationsDir)
    const result: MigrationResult = {
      applied: [],
      skipped: []
    }

    for (const migrationName of migrationFiles) {
      if (applied.has(migrationName)) {
        result.skipped.push(migrationName)
        continue
      }

      const sqlText = readFileSync(join(migrationsDir, migrationName), 'utf8').trim()

      if (sqlText === '') {
        throw new Error(`Migration file is empty: ${migrationName}`)
      }

      await applyMigration(pool, migrationName, sqlText)
      result.applied.push(migrationName)
    }

    return result
  } finally {
    await pool.close()
  }
}

export {
  readMigrationFiles,
  runMigrations,
  splitSqlBatches
}
