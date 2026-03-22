import process from 'node:process'
import Fastify from 'fastify'
import app from './app'

const host = process.env.ADE_API_HOST ?? '127.0.0.1'
const port = Number.parseInt(process.env.ADE_API_PORT ?? '8001', 10)
const server = Fastify({
  logger: true
})
let shuttingDown = false

async function stop(exitCode: number) {
  if (shuttingDown) {
    return
  }

  shuttingDown = true

  try {
    await server.close()
  } finally {
    process.exit(exitCode)
  }
}

async function start() {
  try {
    await server.register(app)
    await server.listen({ host, port })
  } catch (error) {
    console.error(error)
    await stop(1)
  }
}

process.on('SIGINT', () => {
  void stop(0)
})

process.on('SIGTERM', () => {
  void stop(0)
})

void start()
