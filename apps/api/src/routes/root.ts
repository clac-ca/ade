import { FastifyPluginAsync } from 'fastify'

const root: FastifyPluginAsync = async (fastify): Promise<void> => {
  fastify.get('/', async () => {
    return {
      service: 'ade-api',
      status: 'ok'
    }
  })
}

export default root
