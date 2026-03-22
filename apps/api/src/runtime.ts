import { FastifyInstance } from 'fastify'
import { createApp } from './app'
import { BundledBuildInfo } from './config'

export type RuntimeOptions = {
  buildInfo: BundledBuildInfo,
  host: string,
  logger?: boolean,
  port: number
}

export type Runtime = {
  app: FastifyInstance,
  readiness: {
    isReady: boolean
  },
  start: () => Promise<void>,
  stop: () => Promise<void>
}

function createRuntime({ buildInfo, host, logger = true, port }: RuntimeOptions): Runtime {
  const readiness = {
    isReady: false
  }
  const app = createApp({
    buildInfo,
    logger,
    readiness
  })

  async function start() {
    await app.listen({
      host,
      port
    })
    readiness.isReady = true
  }

  async function stop() {
    readiness.isReady = false
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
