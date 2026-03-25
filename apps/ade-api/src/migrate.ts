import process from 'node:process'
import { readConfig } from './config'
import { runMigrations } from './db/migrate-runner'

async function main() {
  const config = readConfig()

  if (!config.sqlConnectionString) {
    throw new Error('Missing required environment variable: AZURE_SQL_CONNECTIONSTRING')
  }

  const result = await runMigrations({
    connectionString: config.sqlConnectionString,
    runtimePrincipalName: process.env.ADE_SQL_RUNTIME_PRINCIPAL_NAME?.trim() || undefined
  })

  for (const migrationName of result.applied) {
    console.log(`Applied migration: ${migrationName}`)
  }

  for (const migrationName of result.skipped) {
    console.log(`Skipped migration: ${migrationName}`)
  }

  console.log(
    `Migration complete. Applied ${result.applied.length}, skipped ${result.skipped.length}.`
  )
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
