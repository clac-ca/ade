import * as assert from 'node:assert'
import { test } from 'node:test'
import { join } from 'node:path'
import {
  DatabaseService,
  DbExecutionResult,
  DbTransaction
} from '../src/db/service'
import { createRuntime } from '../src/runtime'

const webRoot = join(__dirname, 'fixtures', 'web-dist')
const buildInfo = {
  builtAt: '2026-03-21T00:00:00.000Z',
  gitSha: 'test-git-sha',
  service: 'ade' as const,
  version: 'test-version'
}

function createFakeDatabaseService(options: {
  close?: () => Promise<void>,
  ping?: () => Promise<void>
} = {}): DatabaseService {
  const execute = async (): Promise<DbExecutionResult> => ({
    rowsAffected: []
  })
  const query = async <T>(): Promise<readonly T[]> => []
  const tx: DbTransaction = {
    execute,
    query
  }

  return {
    close: options.close ?? (async () => undefined),
    execute,
    ping: options.ping ?? (async () => undefined),
    query,
    withTransaction: async <T>(fn: (transaction: DbTransaction) => Promise<T>) => fn(tx)
  }
}

test('runtime toggles readiness during start and stop', async (t) => {
  let closed = false
  const databaseService = createFakeDatabaseService({
    close: async () => {
      closed = true
    }
  })
  const runtime = createRuntime({
    buildInfo,
    host: '127.0.0.1',
    logger: false,
    port: 0,
    sqlConnectionString: 'Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true',
    sqlServiceFactory: async () => databaseService,
    webRoot
  })

  t.after(async () => {
    if (runtime.app.server.listening) {
      await runtime.stop()
    }
  })

  await runtime.app.ready()
  assert.equal(runtime.app.db, databaseService)

  const beforeStart = await runtime.app.inject({
    url: '/api/readyz'
  })
  assert.equal(beforeStart.statusCode, 503)
  assert.equal(runtime.readiness.isStarted, false)

  await runtime.start()
  assert.equal(runtime.readiness.isStarted, true)

  const address = runtime.app.server.address()
  assert.ok(address && typeof address !== 'string')

  const afterStart = await fetch(`http://127.0.0.1:${address.port}/api/readyz`)
  assert.equal(afterStart.status, 200)

  await runtime.stop()
  assert.equal(runtime.readiness.isStarted, false)
  assert.equal(closed, true)
})

test('runtime startup fails when the database cannot be created', async () => {
  const runtime = createRuntime({
    buildInfo,
    host: '127.0.0.1',
    logger: false,
    port: 0,
    sqlConnectionString: 'Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true',
    sqlServiceFactory: async () => {
      throw new Error('connect failed')
    },
    webRoot
  })

  await assert.rejects(
    () => runtime.start(),
    /connect failed/
  )
})

test('runtime startup closes the database service when the startup ping fails', async () => {
  let closeCount = 0
  const runtime = createRuntime({
    buildInfo,
    host: '127.0.0.1',
    logger: false,
    port: 0,
    sqlConnectionString: 'Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true',
    sqlServiceFactory: async () =>
      createFakeDatabaseService({
        close: async () => {
          closeCount += 1
        },
        ping: async () => {
          throw new Error('ping failed')
        }
      }),
    webRoot
  })

  await assert.rejects(
    () => runtime.start(),
    /Failed to verify SQL connectivity during startup/
  )

  assert.equal(closeCount, 1)
  await runtime.app.close()
  assert.equal(closeCount, 1)
})

test('runtime readiness drops when cached SQL probes fail', async (t) => {
  let shouldFail = false
  const runtime = createRuntime({
    buildInfo,
    host: '127.0.0.1',
    logger: false,
    port: 0,
    probeIntervalMs: 20,
    sqlConnectionString: 'Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true',
    sqlServiceFactory: async () =>
      createFakeDatabaseService({
        ping: async () => {
          if (shouldFail) {
            throw new Error('sql unavailable')
          }
        }
      }),
    staleAfterMs: 80,
    webRoot
  })

  t.after(async () => {
    if (runtime.app.server.listening) {
      await runtime.stop()
    }
  })

  await runtime.start()

  const healthy = await runtime.app.inject({
    url: '/api/readyz'
  })
  assert.equal(healthy.statusCode, 200)

  shouldFail = true

  const deadline = Date.now() + 1_000
  let notReadyStatus = 200

  while (Date.now() < deadline) {
    const res = await runtime.app.inject({
      url: '/api/readyz'
    })
    notReadyStatus = res.statusCode

    if (res.statusCode === 503) {
      break
    }

    await new Promise((resolve) => setTimeout(resolve, 25))
  }

  assert.equal(notReadyStatus, 503)

  const health = await runtime.app.inject({
    url: '/api/healthz'
  })
  assert.equal(health.statusCode, 200)
})
