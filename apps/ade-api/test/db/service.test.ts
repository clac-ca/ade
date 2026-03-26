import * as assert from 'node:assert'
import { test } from 'node:test'
import sql from 'mssql'
import {
  createDatabaseService,
  DatabaseError,
  DbExecutionResult
} from '../../src/db/service'

type RequestCall = {
  args: unknown[],
  kind: 'input' | 'query',
  statement?: string
}

function createFakePool(options: {
  close?: () => Promise<void>,
  queryError?: Error,
  queryResult?: {
    recordset: readonly unknown[],
    rowsAffected: readonly number[]
  },
  transaction?: {
    beginError?: Error,
    commitError?: Error
  }
} = {}) {
  const requestCalls: RequestCall[] = []
  let closeCount = 0
  let rollbackCount = 0
  let commitCount = 0
  let beginCount = 0

  function createRequest() {
    return {
      input(name: string, ...args: unknown[]) {
        requestCalls.push({
          args: [name, ...args],
          kind: 'input'
        })
        return this
      },
      async query<T>(statement: string) {
        requestCalls.push({
          args: [],
          kind: 'query',
          statement
        })

        if (options.queryError) {
          throw options.queryError
        }

        return {
          recordset: (options.queryResult?.recordset ?? []) as readonly T[],
          rowsAffected: options.queryResult?.rowsAffected ?? []
        }
      }
    }
  }

  const pool = {
    async close() {
      closeCount += 1
      await options.close?.()
    },
    request: createRequest,
    transaction() {
      return {
        async begin() {
          beginCount += 1

          if (options.transaction?.beginError) {
            throw options.transaction.beginError
          }
        },
        async commit() {
          commitCount += 1

          if (options.transaction?.commitError) {
            throw options.transaction.commitError
          }
        },
        request: createRequest,
        async rollback() {
          rollbackCount += 1
        }
      }
    }
  }

  return {
    beginCount: () => beginCount,
    closeCount: () => closeCount,
    commitCount: () => commitCount,
    pool,
    requestCalls,
    rollbackCount: () => rollbackCount
  }
}

test('query binds named parameters safely', async () => {
  const fake = createFakePool({
    queryResult: {
      recordset: [{ answer: 42 }],
      rowsAffected: []
    }
  })
  const service = await createDatabaseService(
    'unused',
    async () => fake.pool
  )

  const result = await service.query<{ answer: number }>(
    'SELECT @value AS answer',
    {
      value: 42
    }
  )

  assert.deepStrictEqual(result, [{ answer: 42 }])
  assert.deepStrictEqual(fake.requestCalls, [
    {
      args: ['value', 42],
      kind: 'input'
    },
    {
      args: [],
      kind: 'query',
      statement: 'SELECT @value AS answer'
    }
  ])
})

test('execute forwards explicit typed parameters', async () => {
  const fake = createFakePool({
    queryResult: {
      recordset: [],
      rowsAffected: [1]
    }
  })
  const service = await createDatabaseService(
    'unused',
    async () => fake.pool
  )

  const result = await service.execute(
    'UPDATE dbo.example SET total = @total',
    {
      total: {
        type: sql.Decimal(10, 2),
        value: 12.5
      }
    }
  )

  assert.deepStrictEqual(result, {
    rowsAffected: [1]
  } satisfies DbExecutionResult)
  assert.deepStrictEqual(fake.requestCalls, [
    {
      args: ['total', sql.Decimal(10, 2), 12.5],
      kind: 'input'
    },
    {
      args: [],
      kind: 'query',
      statement: 'UPDATE dbo.example SET total = @total'
    }
  ])
})

test('withTransaction commits on success', async () => {
  const fake = createFakePool({
    queryResult: {
      recordset: [{ ok: true }],
      rowsAffected: []
    }
  })
  const service = await createDatabaseService(
    'unused',
    async () => fake.pool
  )

  const result = await service.withTransaction(async (tx) => {
    const rows = await tx.query<{ ok: boolean }>('SELECT 1 AS ok')
    return rows[0]?.ok ?? false
  })

  assert.equal(result, true)
  assert.equal(fake.beginCount(), 1)
  assert.equal(fake.commitCount(), 1)
  assert.equal(fake.rollbackCount(), 0)
})

test('withTransaction rolls back on failure', async () => {
  const fake = createFakePool()
  const service = await createDatabaseService(
    'unused',
    async () => fake.pool
  )

  await assert.rejects(
    () =>
      service.withTransaction(async () => {
        throw new Error('boom')
      }),
    /boom/
  )

  assert.equal(fake.beginCount(), 1)
  assert.equal(fake.commitCount(), 0)
  assert.equal(fake.rollbackCount(), 1)
})

test('runtime SQL errors are wrapped consistently', async () => {
  const fake = createFakePool({
    queryError: new Error('low-level failure')
  })
  const service = await createDatabaseService(
    'unused',
    async () => fake.pool
  )

  await assert.rejects(
    () => service.query('SELECT 1'),
    (error) => {
      assert.ok(error instanceof DatabaseError)
      assert.equal(error.message, 'SQL query failed.')
      return true
    }
  )
})
