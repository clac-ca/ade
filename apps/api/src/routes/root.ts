import process from 'node:process'
import { FastifyPluginAsync } from 'fastify'
import { BundledBuildInfo } from '../config'

export type RootRouteOptions = {
  buildInfo: BundledBuildInfo,
  readiness: {
    isReady: boolean
  }
}

const root: FastifyPluginAsync<RootRouteOptions> = async (fastify, options): Promise<void> => {
  fastify.get('/', async () => {
    return {
      service: 'ade',
      status: 'ok',
      version: options.buildInfo.version
    }
  })

  fastify.get('/healthz', async () => {
    return {
      service: 'ade',
      status: 'ok'
    }
  })

  fastify.get('/readyz', async (_, reply) => {
    if (!options.readiness.isReady) {
      reply.status(503)
      return {
        service: 'ade',
        status: 'not-ready'
      }
    }

    return {
      service: 'ade',
      status: 'ready'
    }
  })

  fastify.get('/version', async () => {
    return {
      ...options.buildInfo,
      nodeVersion: process.version
    }
  })
}

export default root
