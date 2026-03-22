import { fileURLToPath } from 'node:url'
import process from 'node:process'
import { setTimeout as delay } from 'node:timers/promises'
import { spawnCommand, waitForReady } from './shared.mjs'

const pnpmCommand = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm'

async function waitForExit(child, timeoutMs = 15_000) {
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

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return
  }

  child.kill('SIGINT')
  await waitForExit(child)

  if (child.exitCode === null && child.signalCode === null) {
    child.kill('SIGKILL')
    await waitForExit(child, 1_000)
  }
}

async function assertOk(url) {
  const response = await fetch(url)

  if (!response.ok) {
    throw new Error(`Expected ${url} to return 200, received ${response.status}.`)
  }
}

async function main() {
  const rootDir = fileURLToPath(new URL('..', import.meta.url))
  const port = 8010
  const appUrl = `http://localhost:${port}`
  const child = spawnCommand(
    pnpmCommand,
    ['run', 'start', '--', '--port', String(port), '--no-open'],
    {
      cwd: rootDir
    }
  )

  try {
    await waitForReady(
      [
        `${appUrl}/`,
        `${appUrl}/api/healthz`
      ],
      {
        timeoutMs: 120_000,
        isAlive: () => child.exitCode === null && child.signalCode === null
      }
    )

    await assertOk(`${appUrl}/`)
    await assertOk(`${appUrl}/api/healthz`)
  } finally {
    await stopChild(child)
  }
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
