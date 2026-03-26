import 'fastify'
import { DatabaseService } from './db/service'

declare module 'fastify' {
  interface FastifyInstance {
    db: DatabaseService
  }
}

export {}
