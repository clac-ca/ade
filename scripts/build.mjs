import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import process from 'node:process'
import { spawnCommand } from './shared.mjs'

const dockerCommand = process.platform === 'win32' ? 'docker.exe' : 'docker'
const pnpmCommand = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm'
const rootDir = fileURLToPath(new URL('..', import.meta.url))
const apiPackage = JSON.parse(
  readFileSync(new URL('../apps/api/package.json', import.meta.url), 'utf8')
)

async function runCommand(command, args, options = {}) {
  await new Promise((resolve, reject) => {
    const child = spawnCommand(command, args, {
      cwd: options.cwd ?? rootDir,
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

function readGitSha() {
  if (process.env.GITHUB_SHA) {
    return process.env.GITHUB_SHA
  }

  try {
    return execFileSync('git', ['rev-parse', 'HEAD'], {
      cwd: rootDir,
      encoding: 'utf8'
    }).trim()
  } catch {
    return 'local'
  }
}

async function ensureDocker() {
  try {
    await runCommand(dockerCommand, ['info'], {
      stdio: 'ignore'
    })
  } catch {
    throw new Error('Docker is required for `pnpm build`, and the Docker daemon must be running.')
  }
}

async function main() {
  const builtAt = new Date().toISOString()
  const gitSha = readGitSha()

  await ensureDocker()
  await runCommand(pnpmCommand, ['run', 'build:web'])
  await runCommand(pnpmCommand, ['run', 'build:api'])
  await runCommand(pnpmCommand, ['run', 'package:api'])
  await runCommand(dockerCommand, ['build', '-t', 'ade-web:local', 'apps/web'])
  await runCommand(dockerCommand, [
    'build',
    '-t',
    'ade-api:local',
    '--build-arg',
    `ADE_BUILD_GIT_SHA=${gitSha}`,
    '--build-arg',
    `ADE_BUILD_TIMESTAMP=${builtAt}`,
    '--build-arg',
    `ADE_BUILD_VERSION=${apiPackage.version}`,
    'apps/api'
  ])
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
