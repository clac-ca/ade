import sql from 'mssql'
import { connectToRuntimeSql } from './connection'

type SqlType = (() => sql.ISqlType) | sql.ISqlType
type QueryResultLike<T> = {
  recordset: readonly T[],
  rowsAffected: readonly number[]
}
type RequestLike = {
  input(name: string, value: unknown): RequestLike,
  input(name: string, type: SqlType, value: unknown): RequestLike,
  query<T>(statement: string): Promise<QueryResultLike<T>>
}
type TransactionLike = {
  begin(): Promise<unknown>,
  commit(): Promise<void>,
  rollback(): Promise<void>,
  request(): RequestLike
}
type ConnectionPoolLike = {
  close(): Promise<void>,
  request(): RequestLike,
  transaction(): TransactionLike
}

export type DbScalar =
  | string
  | number
  | boolean
  | bigint
  | Date
  | Buffer
  | Uint8Array
  | null

export type DbTypedParam = {
  type: SqlType,
  value: unknown
}

export type DbParam = DbScalar | DbTypedParam
export type DbParams = Record<string, DbParam>

export type DbExecutionResult = {
  rowsAffected: readonly number[]
}

export type DbTransaction = {
  execute(statement: string, params?: DbParams): Promise<DbExecutionResult>,
  query<T>(statement: string, params?: DbParams): Promise<readonly T[]>
}

export type DatabaseService = DbTransaction & {
  close(): Promise<void>,
  ping(): Promise<void>,
  withTransaction<T>(fn: (tx: DbTransaction) => Promise<T>): Promise<T>
}

export type DatabaseServiceFactory = (
  connectionString: string
) => Promise<DatabaseService>

export type ConnectToRuntimeSqlLike = (
  connectionString: string
) => Promise<ConnectionPoolLike>

class DatabaseError extends Error {
  constructor(message: string, cause?: unknown) {
    super(message, {
      cause
    })
    this.name = 'DatabaseError'
  }
}

function isTypedParam(value: DbParam): value is DbTypedParam {
  return typeof value === 'object' && value !== null && 'type' in value && 'value' in value
}

function normalizeParamValue(value: unknown): unknown {
  if (value instanceof Uint8Array && !Buffer.isBuffer(value)) {
    return Buffer.from(value)
  }

  return value
}

function wrapDatabaseError(message: string, error: unknown): DatabaseError {
  if (error instanceof DatabaseError) {
    return error
  }

  return new DatabaseError(message, error)
}

function bindParams(request: RequestLike, params: DbParams = {}): RequestLike {
  for (const [name, param] of Object.entries(params)) {
    if (isTypedParam(param)) {
      request.input(name, param.type, normalizeParamValue(param.value))
      continue
    }

    request.input(name, normalizeParamValue(param))
  }

  return request
}

function createRequestExecutor(createRequest: () => RequestLike): DbTransaction {
  return {
    async execute(statement: string, params: DbParams = {}) {
      try {
        const result = await bindParams(createRequest(), params).query(statement)

        return {
          rowsAffected: result.rowsAffected
        }
      } catch (error) {
        throw wrapDatabaseError('SQL command failed.', error)
      }
    },
    async query<T>(statement: string, params: DbParams = {}) {
      try {
        const result = await bindParams(createRequest(), params).query<T>(statement)
        return result.recordset
      } catch (error) {
        throw wrapDatabaseError('SQL query failed.', error)
      }
    }
  }
}

async function createDatabaseService(
  connectionString: string,
  connect: ConnectToRuntimeSqlLike = connectToRuntimeSql
): Promise<DatabaseService> {
  let pool: ConnectionPoolLike

  try {
    pool = await connect(connectionString)
  } catch (error) {
    throw wrapDatabaseError('Failed to connect to SQL.', error)
  }

  const rootExecutor = createRequestExecutor(() => pool.request())

  return {
    ...rootExecutor,
    async close() {
      try {
        await pool.close()
      } catch (error) {
        throw wrapDatabaseError('Failed to close the SQL pool.', error)
      }
    },
    async ping() {
      try {
        await pool.request().query('SELECT 1 AS value')
      } catch (error) {
        throw wrapDatabaseError('SQL ping failed.', error)
      }
    },
    async withTransaction<T>(fn: (tx: DbTransaction) => Promise<T>) {
      const transaction = pool.transaction()

      try {
        await transaction.begin()
      } catch (error) {
        throw wrapDatabaseError('Failed to begin SQL transaction.', error)
      }

      const txExecutor = createRequestExecutor(() => transaction.request())

      try {
        const result = await fn(txExecutor)

        try {
          await transaction.commit()
        } catch (error) {
          throw wrapDatabaseError('Failed to commit SQL transaction.', error)
        }

        return result
      } catch (error) {
        await transaction.rollback().catch(() => undefined)
        throw error
      }
    }
  }
}

export {
  createDatabaseService,
  DatabaseError
}
