import { spawn } from 'node:child_process'
import { createServer } from 'node:net'
import process from 'node:process'
import { setTimeout as delay } from 'node:timers/promises'

export function parseArgs(argv, options = {}) {
  const {
    defaultPort,
    allowNoOpen = false
  } = options

  let port = defaultPort
  let noOpen = false

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]

    if (arg === '--') {
      continue
    }

    if (arg === '--port') {
      const value = argv[index + 1]

      if (value === undefined) {
        throw new Error('Missing value for --port')
      }

      if (!/^[1-9]\d*$/.test(value)) {
        throw new Error(`Invalid port: ${value}`)
      }

      const parsed = Number.parseInt(value, 10)

      if (parsed > 65_535) {
        throw new Error(`Invalid port: ${value}`)
      }

      port = parsed
      index += 1
      continue
    }

    if (allowNoOpen && arg === '--no-open') {
      noOpen = true
      continue
    }

    throw new Error(`Unknown argument: ${arg}`)
  }

  return {
    port,
    noOpen
  }
}

export function spawnCommand(command, args, options = {}) {
  const child = spawn(command, args, {
    cwd: options.cwd,
    detached: options.detached ?? false,
    env: {
      ...process.env,
      ...options.env
    },
    stdio: options.stdio ?? 'inherit'
  })

  child.adeDetached = options.detached ?? false

  return child
}

export async function runCommand(command, args, options = {}) {
  await new Promise((resolve, reject) => {
    const child = spawnCommand(command, args, {
      cwd: options.cwd,
      detached: options.detached,
      env: options.env,
      stdio: options.stdio ?? 'inherit'
    })

    child.on('error', reject)
    child.on('exit', (code, signal) => {
      if (signal !== null) {
        reject(new Error(`${command} exited with signal ${signal}`))
        return
      }

      if (code !== 0) {
        reject(new Error(`${command} exited with code ${code ?? 'unknown'}`))
        return
      }

      resolve(undefined)
    })
  })
}

export async function waitForReady(urls, options = {}) {
  const timeoutMs = options.timeoutMs ?? 30_000
  const startedAt = Date.now()

  while (Date.now() - startedAt < timeoutMs) {
    if (options.isAlive && !options.isAlive()) {
      throw new Error('A required process exited before ADE became ready.')
    }

    const checks = await Promise.all(
      urls.map(async (url) => {
        try {
          const response = await fetch(url)
          return response.ok
        } catch {
          return false
        }
      })
    )

    if (checks.every(Boolean)) {
      return
    }

    await delay(250)
  }

  throw new Error(`Timed out waiting for: ${urls.join(', ')}`)
}

export async function findAvailablePort() {
  return await new Promise((resolve, reject) => {
    const server = createServer()

    server.on('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()

      if (!address || typeof address === 'string') {
        server.close(() => reject(new Error('Could not determine a free port.')))
        return
      }

      server.close((error) => {
        if (error) {
          reject(error)
          return
        }

        resolve(address.port)
      })
    })
  })
}

export function registerShutdown(handler) {
  let shuttingDown = false

  const run = async (exitCode = 0) => {
    if (shuttingDown) {
      return
    }

    shuttingDown = true

    try {
      await handler()
    } finally {
      process.exit(exitCode)
    }
  }

  process.on('SIGINT', () => {
    void run(0)
  })

  process.on('SIGTERM', () => {
    void run(0)
  })

  process.on('uncaughtException', (error) => {
    console.error(error)
    void run(1)
  })

  process.on('unhandledRejection', (error) => {
    console.error(error)
    void run(1)
  })

  return run
}

export function openBrowser(url) {
  const platform = process.platform

  if (platform === 'darwin') {
    const child = spawn('open', [url], {
      detached: true,
      stdio: 'ignore'
    })
    child.on('error', () => {})
    child.unref()
    return
  }

  if (platform === 'win32') {
    const child = spawn('cmd', ['/c', 'start', '', url], {
      detached: true,
      stdio: 'ignore'
    })
    child.on('error', () => {})
    child.unref()
    return
  }

  const child = spawn('xdg-open', [url], {
    detached: true,
    stdio: 'ignore'
  })
  child.on('error', () => {})
  child.unref()
}
