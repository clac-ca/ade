import { FastifyPluginAsync } from 'fastify'
import { BuildInfo } from '../config'

export type RootRouteOptions = {
  buildInfo: BuildInfo,
  readiness: {
    isReady: boolean
  }
}

const root: FastifyPluginAsync<RootRouteOptions> = async (fastify, options): Promise<void> => {
  fastify.get('/', async () => {
    return {
      service: 'ade-api',
      status: 'ok',
      version: options.buildInfo.version
    }
  })

  fastify.get('/healthz', async () => {
    return {
      service: 'ade-api',
      status: 'ok'
    }
  })

  fastify.get('/readyz', async (_, reply) => {
    if (!options.readiness.isReady) {
      reply.status(503)
      return {
        service: 'ade-api',
        status: 'not-ready'
      }
    }

    return {
      service: 'ade-api',
      status: 'ready'
    }
  })

  fastify.get('/version', async () => {
    return options.buildInfo
  })
}

export default root
