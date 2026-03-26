import { buildConnectionString, parseSqlConnectionString as parseTediousSqlConnectionString } from '@tediousjs/connection-string'
import sql from 'mssql'

export type ParsedSqlConnectionString = {
  authentication: 'managed-identity' | 'sql-password',
  database: string,
  encrypt: boolean,
  password?: string,
  port: number,
  server: string,
  trustServerCertificate: boolean,
  userId?: string
}

type SqlBatchConnection = {
  close(): Promise<void>,
  request(): {
    batch(statement: string): Promise<unknown>
  }
}

type ConnectToSqlLike = (connectionString: string) => Promise<SqlBatchConnection>
type ConnectionFields = Record<string, string | number | boolean | null | undefined>

const DEFAULT_SQL_PORT = 1433

function parseServer(value: string): {
  port: number,
  server: string
} {
  const trimmed = value.trim()

  if (trimmed === '') {
    throw new Error('SQL connection string must include a server.')
  }

  const withoutProtocol = trimmed.replace(/^tcp:/i, '')
  const lastComma = withoutProtocol.lastIndexOf(',')

  if (lastComma === -1) {
    return {
      port: DEFAULT_SQL_PORT,
      server: withoutProtocol
    }
  }

  const server = withoutProtocol.slice(0, lastComma).trim()
  const portValue = withoutProtocol.slice(lastComma + 1).trim()

  if (!/^[1-9]\d*$/.test(portValue)) {
    throw new Error(`Invalid SQL Server port: ${portValue}`)
  }

  const port = Number.parseInt(portValue, 10)

  return {
    port,
    server
  }
}

function parseConnectionFields(connectionString: string): ConnectionFields {
  return parseTediousSqlConnectionString(connectionString, true)
}

function readOptionalStringField(fields: ConnectionFields, name: string): string | undefined {
  const value = fields[name]

  if (value === undefined) {
    return undefined
  }

  if (typeof value !== 'string') {
    throw new Error(`SQL connection string field "${name}" must be a string.`)
  }

  const trimmed = value.trim()
  return trimmed === '' ? undefined : trimmed
}

function readRequiredField(fields: ConnectionFields, keys: string[]): string {
  for (const key of keys) {
    const value = readOptionalStringField(fields, key)

    if (value !== undefined) {
      return value
    }
  }

  throw new Error(`SQL connection string is missing one of: ${keys.join(', ')}`)
}

function readBooleanField(fields: ConnectionFields, name: string, fallback: boolean): boolean {
  const value = fields[name]

  if (value === undefined) {
    return fallback
  }

  if (typeof value === 'boolean') {
    return value
  }

  throw new Error(`Invalid boolean value in SQL connection string field "${name}": ${String(value)}`)
}

function parseAuthentication(fields: ConnectionFields) {
  const authentication = readOptionalStringField(fields, 'authentication')

  if (authentication === undefined) {
    return 'sql-password'
  }

  const normalized = authentication.trim().toLowerCase().replace(/\s+/g, '')

  if (
    normalized === 'activedirectorymanagedidentity' ||
    normalized === 'activedirectorydefault'
  ) {
    return 'managed-identity'
  }

  if (normalized === 'sqlpassword') {
    return 'sql-password'
  }

  throw new Error(`Unsupported SQL authentication mode: ${authentication}`)
}

function parseSqlConnectionString(connectionString: string): ParsedSqlConnectionString {
  const fields = parseConnectionFields(connectionString)
  const { port, server } = parseServer(readRequiredField(fields, ['data source']))
  const database = readRequiredField(fields, ['initial catalog', 'database'])
  const authentication = parseAuthentication(fields)
  const userId = readOptionalStringField(fields, 'user id')
  const password = readOptionalStringField(fields, 'password')
  const encrypt = readBooleanField(fields, 'encrypt', authentication === 'managed-identity')
  const trustServerCertificate = readBooleanField(
    fields,
    'trustservercertificate',
    authentication !== 'managed-identity'
  )

  if (authentication === 'sql-password' && (!userId || !password)) {
    throw new Error('SQL authentication requires both User ID and Password.')
  }

  return {
    authentication,
    database,
    encrypt,
    password,
    port,
    server,
    trustServerCertificate,
    userId
  }
}

function buildPoolConfig(connectionString: string): sql.config {
  const parsed = parseSqlConnectionString(connectionString)

  if (parsed.authentication === 'managed-identity') {
    return {
      authentication: {
        options: parsed.userId
          ? {
              clientId: parsed.userId
            }
          : {},
        type: 'azure-active-directory-default'
      },
      database: parsed.database,
      options: {
        encrypt: parsed.encrypt,
        trustServerCertificate: parsed.trustServerCertificate
      },
      port: parsed.port,
      server: parsed.server
    }
  }

  return {
    database: parsed.database,
    options: {
      encrypt: parsed.encrypt,
      trustServerCertificate: parsed.trustServerCertificate
    },
    password: parsed.password,
    port: parsed.port,
    server: parsed.server,
    user: parsed.userId
  }
}

async function connectWithConfig(config: sql.config): Promise<sql.ConnectionPool> {
  const pool = new sql.ConnectionPool(config)

  try {
    return await pool.connect()
  } catch (error) {
    await pool.close().catch(() => undefined)
    throw error
  }
}

async function connectToSql(connectionString: string): Promise<sql.ConnectionPool> {
  return connectWithConfig(buildPoolConfig(connectionString))
}

async function connectToRuntimeSql(connectionString: string): Promise<sql.ConnectionPool> {
  return connectWithConfig(buildPoolConfig(connectionString))
}

async function ensureDatabaseExists(
  connectionString: string,
  connect: ConnectToSqlLike = connectToSql
): Promise<void> {
  const parsed = parseSqlConnectionString(connectionString)

  if (parsed.authentication !== 'sql-password') {
    return
  }

  const pool = await connect(withDatabase(connectionString, 'master'))

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

function quoteSqlIdentifier(value: string): string {
  return `[${value.replaceAll(']', ']]')}]`
}

function withDatabase(connectionString: string, database: string): string {
  const fields = parseConnectionFields(connectionString)
  fields['initial catalog'] = database
  delete fields.database

  return buildConnectionString(fields)
}

export {
  buildPoolConfig,
  connectToSql,
  connectToRuntimeSql,
  ensureDatabaseExists,
  parseSqlConnectionString,
  quoteSqlIdentifier,
  withDatabase
}
