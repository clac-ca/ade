import { existsSync } from 'node:fs'
import { join } from 'node:path'
import fastifyStatic from '@fastify/static'
import Fastify, { FastifyInstance } from 'fastify'
import rootRoute, { RootRouteOptions } from './routes/root'

export type CreateAppOptions = RootRouteOptions & {
  logger?: boolean,
  webRoot?: string
}

function createApp({ logger = true, webRoot, ...options }: CreateAppOptions): FastifyInstance {
  const server = Fastify({
    logger
  })

  server.register(rootRoute, {
    ...options,
    prefix: '/api'
  })

  if (webRoot && existsSync(join(webRoot, 'index.html'))) {
    server.register(fastifyStatic, {
      root: webRoot
    })

    server.get('/', async (_, reply) => {
      return reply.sendFile('index.html')
    })

    server.setNotFoundHandler(async (request, reply) => {
      const requestPath = request.url.split('?', 1)[0]

      if (requestPath === '/api' || requestPath.startsWith('/api/') || /\.[^/]+$/.test(requestPath)) {
        reply.status(404)
        return {
          error: 'Not Found',
          message: `Route ${request.method}:${requestPath} not found`,
          statusCode: 404
        }
      }

      return reply.sendFile('index.html')
    })
  }

  return server
}

export {
  createApp
}
