import { FastifyInstance } from 'fastify'
import { createApp } from './app'
import { DatabaseServiceFactory } from './db/service'
import { BundledBuildInfo } from './config'
import { createReadinessState, ReadinessState } from './readiness'

export type RuntimeOptions = {
  buildInfo: BundledBuildInfo,
  host: string,
  logger?: boolean,
  port: number,
  probeIntervalMs?: number,
  sqlConnectionString: string,
  sqlServiceFactory?: DatabaseServiceFactory,
  staleAfterMs?: number,
  webRoot?: string
}

export type Runtime = {
  app: FastifyInstance,
  readiness: ReadinessState,
  start: () => Promise<void>,
  stop: () => Promise<void>
}

function createRuntime({
  buildInfo,
  host,
  logger = true,
  port,
  probeIntervalMs,
  sqlConnectionString,
  sqlServiceFactory,
  staleAfterMs,
  webRoot
}: RuntimeOptions): Runtime {
  const readiness = createReadinessState({
    staleAfterMs
  })
  const app = createApp({
    buildInfo,
    database: {
      connectionString: sqlConnectionString,
      createService: sqlServiceFactory,
      probeIntervalMs,
      readiness
    },
    logger,
    readiness,
    webRoot
  })

  async function start() {
    await app.listen({
      host,
      port
    })
    readiness.isStarted = true
  }

  async function stop() {
    readiness.isStarted = false
    await app.close()
  }

  return {
    app,
    readiness,
    start,
    stop
  }
}

export {
  createRuntime
}
