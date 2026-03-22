import { FastifyPluginAsync } from 'fastify'
import rootRoute from './routes/root'

const app: FastifyPluginAsync = async (fastify): Promise<void> => {
  await fastify.register(rootRoute)
}

export default app
