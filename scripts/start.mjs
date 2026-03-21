import process from 'node:process'
import {
  openBrowser,
  parseArgs,
  registerShutdown,
  spawnPnpm,
  terminateChildren,
  waitForReady
} from './shared.mjs'

const { port, noOpen } = parseArgs(process.argv.slice(2), {
  defaultPort: 8000,
  allowNoOpen: true
})

const api = spawnPnpm(['run', 'dev:api'], {
  detached: true,
  env: {
    ADE_API_HOST: '127.0.0.1',
    ADE_API_PORT: '8001'
  }
})
const web = spawnPnpm(['run', 'dev:web'], {
  detached: true,
  env: {
    ADE_API_ORIGIN: 'http://127.0.0.1:8001',
    ADE_WEB_PORT: String(port)
  }
})

const children = [api, web]
const appUrl = `http://localhost:${port}`

const shutdown = registerShutdown(async () => {
  await terminateChildren(children)
})

for (const child of children) {
  child.on('exit', (code, signal) => {
    if (signal === 'SIGINT' || signal === 'SIGTERM' || signal === 'SIGKILL') {
      return
    }

    console.error(`Launcher child exited with code ${code ?? 'unknown'}${signal ? ` and signal ${signal}` : ''}.`)
    void shutdown(code ?? 1)
  })
}

try {
  await waitForReady(
    [
      'http://127.0.0.1:8001/healthz'
    ],
    {
      isAlive: () => children.every((child) => child.exitCode === null && child.signalCode === null)
    }
  )

  await waitForReady(
    [
      `${appUrl}/`,
      `${appUrl}/api/healthz`
    ],
    {
      isAlive: () => children.every((child) => child.exitCode === null && child.signalCode === null)
    }
  )
} catch (error) {
  console.error(error instanceof Error ? error.message : error)
  await shutdown(1)
  process.exit(1)
}

if (!noOpen) {
  openBrowser(appUrl)
}

console.log(`ADE is running at ${appUrl}`)
