import fp from 'fastify-plugin'
import { FastifyPluginAsync } from 'fastify'
import {
  createDatabaseService,
  DatabaseError,
  DatabaseService,
  DatabaseServiceFactory
} from '../db/service'
import { ReadinessState } from '../readiness'

export type DatabasePluginOptions = {
  connectionString: string,
  createService?: DatabaseServiceFactory,
  probeIntervalMs?: number,
  readiness: ReadinessState
}

const databasePlugin: FastifyPluginAsync<DatabasePluginOptions> = async (
  fastify,
  options
): Promise<void> => {
  fastify.decorate('db', null as unknown as DatabaseService)

  const createService = options.createService ?? createDatabaseService
  const probeIntervalMs = options.probeIntervalMs ?? 5_000
  let service: DatabaseService | undefined
  let serviceClosed = false
  let probeInFlight: Promise<void> | null = null
  let probeInterval: NodeJS.Timeout | null = null
  let closed = false

  async function closeService(): Promise<void> {
    if (!service || serviceClosed) {
      return
    }

    serviceClosed = true
    await service.close()
    fastify.log.info('SQL disconnected.')
  }

  fastify.addHook('onClose', async () => {
    closed = true

    if (probeInterval) {
      clearInterval(probeInterval)
    }

    await probeInFlight?.catch(() => undefined)
    await closeService()
  })

  function updateReadiness(ok: boolean, error?: unknown) {
    const previousOk = options.readiness.database.ok
    const hadPreviousCheck = options.readiness.database.lastCheckedAt !== null
    const message =
      error instanceof Error
        ? error.message
        : typeof error === 'string'
          ? error
          : null

    options.readiness.database.ok = ok
    options.readiness.database.lastCheckedAt = Date.now()
    options.readiness.database.lastError = ok ? null : message

    if (!ok && previousOk) {
      fastify.log.error(
        {
          err: error
        },
        'SQL readiness probe failed.'
      )
    }

    if (ok && hadPreviousCheck && !previousOk) {
      fastify.log.info('SQL readiness probe recovered.')
    }
  }

  async function runProbe(source: 'startup' | 'interval'): Promise<void> {
    if (closed) {
      return
    }

    if (probeInFlight) {
      return probeInFlight
    }

    probeInFlight = (async () => {
      try {
        await fastify.db.ping()
        updateReadiness(true)
      } catch (error) {
        updateReadiness(false, error)

        if (source === 'startup') {
          throw new DatabaseError('Failed to verify SQL connectivity during startup.', error)
        }
      } finally {
        probeInFlight = null
      }
    })()

    return probeInFlight
  }

  try {
    service = await createService(options.connectionString)
    fastify.db = service
    await runProbe('startup')
    fastify.log.info('SQL connected.')
  } catch (error) {
    await closeService().catch((closeError) => {
      fastify.log.error(
        {
          err: closeError
        },
        'Failed to close SQL after startup failure.'
      )
    })

    fastify.log.error(
      {
        err: error
      },
      'Failed to initialize SQL.'
    )
    throw error
  }

  probeInterval = setInterval(() => {
    void runProbe('interval')
  }, probeIntervalMs)
  probeInterval.unref?.()
}

export default fp(databasePlugin, {
  name: 'database'
})
