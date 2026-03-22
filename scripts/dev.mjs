import process from 'node:process'
import { setTimeout as delay } from 'node:timers/promises'
import {
  openBrowser,
  parseArgs,
  registerShutdown,
  spawnCommand,
  waitForReady
} from './shared.mjs'

const pnpmCommand = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm'

function signalChild(child, signal) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return
  }

  try {
    if (child.adeDetached && process.platform !== 'win32') {
      process.kill(-child.pid, signal)
      return
    }

    child.kill(signal)
  } catch (error) {
    if (!(error instanceof Error) || !error.message.includes('kill ESRCH')) {
      throw error
    }
  }
}

async function terminateChildren(children) {
  for (const child of children) {
    signalChild(child, 'SIGINT')
  }

  await delay(1_000)

  for (const child of children) {
    signalChild(child, 'SIGTERM')
  }

  await delay(250)

  for (const child of children) {
    signalChild(child, 'SIGKILL')
  }
}

async function main() {
  const { port, noOpen } = parseArgs(process.argv.slice(2), {
    defaultPort: 8000,
    allowNoOpen: true
  })

  const detached = process.platform !== 'win32'
  const api = spawnCommand(pnpmCommand, ['--filter', '@ade/api', 'dev'], {
    detached,
    env: {
      ADE_API_HOST: '127.0.0.1',
      ADE_API_PORT: '8001'
    }
  })
  const web = spawnCommand(pnpmCommand, ['--filter', '@ade/web', 'dev'], {
    detached,
    env: {
      ADE_API_ORIGIN: 'http://127.0.0.1:8001',
      ADE_WEB_PORT: String(port)
    }
  })

  const children = [api, web]
  const appUrl = `http://localhost:${port}`
  let shuttingDown = false

  const shutdown = registerShutdown(async () => {
    shuttingDown = true
    await terminateChildren(children)
  })

  for (const child of children) {
    child.on('exit', (code, signal) => {
      if (shuttingDown) {
        return
      }

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
        'http://127.0.0.1:8001/readyz'
      ],
      {
        isAlive: () => children.every((child) => child.exitCode === null && child.signalCode === null)
      }
    )

    await waitForReady(
      [
        `${appUrl}/`,
        `${appUrl}/api/readyz`
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

  console.log(`ADE dev is running at ${appUrl}`)
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
