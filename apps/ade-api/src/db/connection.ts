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

const DEFAULT_SQL_PORT = 1433

function parseBoolean(value: string | undefined, fallback: boolean): boolean {
  if (value === undefined) {
    return fallback
  }

  const normalized = value.trim().toLowerCase()

  if (normalized === 'true' || normalized === 'yes') {
    return true
  }

  if (normalized === 'false' || normalized === 'no') {
    return false
  }

  throw new Error(`Invalid boolean value in SQL connection string: ${value}`)
}

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

  const server = withoutProtocol.slice(0, lastComma)
  const portValue = withoutProtocol.slice(lastComma + 1)

  if (!/^[1-9]\d*$/.test(portValue)) {
    throw new Error(`Invalid SQL Server port: ${portValue}`)
  }

  const port = Number.parseInt(portValue, 10)

  return {
    port,
    server
  }
}

function readRequiredField(fields: Map<string, string>, keys: string[]): string {
  for (const key of keys) {
    const value = fields.get(key)

    if (value !== undefined && value.trim() !== '') {
      return value.trim()
    }
  }

  throw new Error(`SQL connection string is missing one of: ${keys.join(', ')}`)
}

function parseFields(connectionString: string): Map<string, string> {
  const fields = new Map<string, string>()

  for (const segment of connectionString.split(';')) {
    const trimmed = segment.trim()

    if (trimmed === '') {
      continue
    }

    const separatorIndex = trimmed.indexOf('=')

    if (separatorIndex === -1) {
      throw new Error(`Invalid SQL connection string segment: ${trimmed}`)
    }

    const key = trimmed.slice(0, separatorIndex).trim().toLowerCase()
    const value = trimmed.slice(separatorIndex + 1).trim()
    fields.set(key, value)
  }

  return fields
}

function parseAuthentication(fields: Map<string, string>) {
  const authentication =
    fields.get('authentication') ??
    fields.get('authenticationtype')

  if (authentication === undefined || authentication.trim() === '') {
    return 'sql-password'
  }

  const normalized = authentication.trim().toLowerCase().replace(/\s+/g, '')

  if (normalized === 'activedirectorymanagedidentity' || normalized === 'activedirectorydefault') {
    return 'managed-identity'
  }

  throw new Error(`Unsupported SQL authentication mode: ${authentication}`)
}

function parseSqlConnectionString(connectionString: string): ParsedSqlConnectionString {
  const fields = parseFields(connectionString)
  const { port, server } = parseServer(readRequiredField(fields, ['data source', 'server', 'address', 'addr']))
  const database = readRequiredField(fields, ['initial catalog', 'database'])
  const authentication = parseAuthentication(fields)
  const userId = fields.get('user id')?.trim() || fields.get('uid')?.trim() || undefined
  const password = fields.get('password')?.trim() || fields.get('pwd')?.trim() || undefined
  const encrypt = parseBoolean(fields.get('encrypt'), authentication === 'managed-identity')
  const trustServerCertificate = parseBoolean(fields.get('trustservercertificate'), authentication !== 'managed-identity')

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

async function connectToSql(connectionString: string): Promise<sql.ConnectionPool> {
  const pool = new sql.ConnectionPool(buildPoolConfig(connectionString))

  try {
    return await pool.connect()
  } catch (error) {
    await pool.close().catch(() => undefined)
    throw error
  }
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
  const fields = parseFields(connectionString)
  fields.set('database', database)
  fields.delete('initial catalog')

  return Array.from(fields.entries())
    .map(([key, value]) => `${key}=${value}`)
    .join(';')
}

export {
  buildPoolConfig,
  connectToSql,
  ensureDatabaseExists,
  parseSqlConnectionString,
  quoteSqlIdentifier,
  withDatabase
}
