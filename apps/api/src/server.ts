import process from 'node:process'
import { createApp } from './app'
import { readConfig } from './config'

async function main() {
  const config = readConfig()
  const readiness = {
    isReady: false
  }
  const server = createApp({
    buildInfo: config.buildInfo,
    readiness
  })
  let shuttingDown = false

  async function stop(exitCode: number) {
    if (shuttingDown) {
      return
    }

    shuttingDown = true
    readiness.isReady = false

    try {
      await server.close()
    } finally {
      process.exit(exitCode)
    }
  }

  process.on('SIGINT', () => {
    void stop(0)
  })

  process.on('SIGTERM', () => {
    void stop(0)
  })

  try {
    await server.listen({
      host: config.host,
      port: config.port
    })
    readiness.isReady = true
  } catch (error) {
    console.error(error)
    await stop(1)
  }
}

void main().catch((error) => {
  console.error(error)
  process.exit(1)
})
