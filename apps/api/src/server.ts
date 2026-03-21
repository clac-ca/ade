import Fastify, { FastifyInstance } from 'fastify'
import app, { options } from './app'

const host = process.env.ADE_API_HOST ?? '127.0.0.1'
const port = Number.parseInt(process.env.ADE_API_PORT ?? '8001', 10)
let server: FastifyInstance | undefined
let shuttingDown = false

async function stop(exitCode: number) {
  if (shuttingDown) {
    return
  }

  shuttingDown = true

  try {
    await server?.close()
  } finally {
    process.exit(exitCode)
  }
}

async function start() {
  server = Fastify({
    logger: true
  })

  await server.register(app, options)
  await server.listen({ host, port })
}

process.on('SIGINT', () => {
  void stop(0)
})

process.on('SIGTERM', () => {
  void stop(0)
})

void start().catch((error) => {
  console.error(error)
  void stop(1)
})
