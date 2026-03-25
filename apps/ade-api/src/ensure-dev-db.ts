import process from 'node:process'
import { readConfig } from './config'
import { connectToSql, parseSqlConnectionString, quoteSqlIdentifier, withDatabase } from './db/connection'

async function main() {
  const config = readConfig()

  if (!config.sqlConnectionString) {
    throw new Error('Missing required environment variable: AZURE_SQL_CONNECTIONSTRING')
  }

  const parsed = parseSqlConnectionString(config.sqlConnectionString)

  if (parsed.authentication !== 'sql-password') {
    throw new Error('ensure-dev-db only supports SQL authentication connection strings.')
  }

  const masterConnectionString = withDatabase(config.sqlConnectionString, 'master')
  const pool = await connectToSql(masterConnectionString)

  try {
    const databaseIdentifier = quoteSqlIdentifier(parsed.database)

    await pool.request().batch(`
      IF DB_ID(N'${parsed.database.replaceAll("'", "''")}') IS NULL
      BEGIN
        EXEC(N'CREATE DATABASE ${databaseIdentifier}');
      END
    `)
  } finally {
    await pool.close()
  }
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
