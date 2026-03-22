import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import process from 'node:process'
import { runCommand } from './shared.mjs'

const dockerCommand = process.platform === 'win32' ? 'docker.exe' : 'docker'
const pnpmCommand = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm'
const rootDir = fileURLToPath(new URL('..', import.meta.url))
const apiPackage = JSON.parse(
  readFileSync(new URL('../apps/api/package.json', import.meta.url), 'utf8')
)

function readGitMetadata() {
  const gitSha = process.env.GITHUB_SHA ?? readGitValue(['rev-parse', 'HEAD']) ?? 'local'
  const builtAt = readGitValue(['show', '--no-patch', '--format=%cI', gitSha]) ?? new Date().toISOString()

  return {
    builtAt,
    gitSha
  }
}

function readGitValue(args) {
  try {
    return execFileSync('git', args, {
      cwd: rootDir,
      encoding: 'utf8'
    }).trim()
  } catch {
    return null
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
  const { builtAt, gitSha } = readGitMetadata()
  const buildInfoPath = join(rootDir, 'apps', 'api', '.package', 'dist', 'build-info.json')

  await ensureDocker()
  await runCommand(pnpmCommand, ['--filter', '@ade/web', 'build'], {
    cwd: rootDir
  })
  await runCommand(pnpmCommand, ['--filter', '@ade/api', 'build'], {
    cwd: rootDir
  })
  rmSync(join(rootDir, 'apps', 'api', '.package'), {
    force: true,
    recursive: true
  })
  await runCommand(pnpmCommand, ['--filter', '@ade/api', 'deploy', '--prod', 'apps/api/.package'], {
    cwd: rootDir
  })

  mkdirSync(dirname(buildInfoPath), {
    recursive: true
  })
  writeFileSync(buildInfoPath, JSON.stringify({
    builtAt,
    gitSha,
    service: 'ade-api',
    version: apiPackage.version
  }, null, 2) + '\n')

  await runCommand(dockerCommand, ['build', '-t', 'ade-web:local', 'apps/web'])
  await runCommand(dockerCommand, ['build', '-t', 'ade-api:local', 'apps/api'])
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
