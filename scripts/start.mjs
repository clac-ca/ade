import { fileURLToPath } from 'node:url'
import process from 'node:process'
import { setTimeout as delay } from 'node:timers/promises'
import {
  openBrowser,
  parseArgs,
  registerShutdown,
  spawnCommand,
  waitForReady
} from './shared.mjs'

const dockerCommand = process.platform === 'win32' ? 'docker.exe' : 'docker'
const rootDir = fileURLToPath(new URL('..', import.meta.url))
const imageTags = ['ade-web:local', 'ade-api:local']

function composeArgs(projectName, ...args) {
  return ['compose', '--project-name', projectName, ...args]
}

async function runDocker(args, options = {}) {
  await new Promise((resolve, reject) => {
    const child = spawnCommand(dockerCommand, args, {
      cwd: rootDir,
      env: options.env,
      stdio: options.stdio ?? 'inherit'
    })

    child.on('error', reject)
    child.on('exit', (code, signal) => {
      if (signal !== null) {
        reject(new Error(`docker exited with signal ${signal}`))
        return
      }

      if (code !== 0) {
        reject(new Error(`docker exited with code ${code ?? 'unknown'}`))
        return
      }

      resolve(undefined)
    })
  })
}

async function waitForExit(child, timeoutMs = 5_000) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return
  }

  await Promise.race([
    new Promise((resolve) => {
      child.once('exit', () => resolve(undefined))
    }),
    delay(timeoutMs)
  ])
}

async function main() {
  const { port, noOpen } = parseArgs(process.argv.slice(2), {
    defaultPort: 8000,
    allowNoOpen: true
  })

  const appUrl = `http://localhost:${port}`
  const projectName = `ade-local-${port}`

  async function ensureDocker() {
    try {
      await runDocker(['info'], {
        stdio: 'ignore'
      })
    } catch {
      throw new Error('Docker is required for `pnpm start`, and the Docker daemon must be running.')
    }
  }

  async function ensureImages() {
    try {
      await runDocker(['image', 'inspect', ...imageTags], {
        stdio: 'ignore'
      })
    } catch {
      throw new Error('Run pnpm build first.')
    }
  }

  async function composeDown() {
    try {
      await runDocker(composeArgs(projectName, 'down', '--remove-orphans'), {
        stdio: 'ignore'
      })
    } catch {}
  }

  await ensureDocker()
  await ensureImages()
  await composeDown()

  const compose = spawnCommand(dockerCommand, composeArgs(projectName, 'up'), {
    cwd: rootDir,
    env: {
      ADE_PORT: String(port)
    }
  })
  let shuttingDown = false

  const shutdown = registerShutdown(async () => {
    shuttingDown = true
    await composeDown()
    compose.kill('SIGINT')
    await waitForExit(compose)

    if (compose.exitCode === null && compose.signalCode === null) {
      compose.kill('SIGKILL')
      await waitForExit(compose, 1_000)
    }
  })

  compose.on('exit', (code, signal) => {
    if (shuttingDown) {
      return
    }

    if (signal === 'SIGINT' || signal === 'SIGTERM' || signal === 'SIGKILL') {
      return
    }

    console.error(`Launcher child exited with code ${code ?? 'unknown'}${signal ? ` and signal ${signal}` : ''}.`)
    void shutdown(code ?? 1)
  })

  try {
    await waitForReady(
      [
        `${appUrl}/`,
        `${appUrl}/api/healthz`
      ],
      {
        timeoutMs: 60_000,
        isAlive: () => compose.exitCode === null && compose.signalCode === null
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
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
