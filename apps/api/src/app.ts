import Fastify, { FastifyInstance } from 'fastify'
import rootRoute, { RootRouteOptions } from './routes/root'

export type CreateAppOptions = RootRouteOptions & {
  logger?: boolean
}

function createApp({ logger = true, ...options }: CreateAppOptions): FastifyInstance {
  const server = Fastify({
    logger
  })

  void server.register(rootRoute, options)

  return server
}

export {
  createApp
}
