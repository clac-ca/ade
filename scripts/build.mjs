import process from 'node:process'
import { runCommand } from './shared.mjs'
import { buildArtifacts } from './build-artifacts.mjs'

const dockerCommand = process.platform === 'win32' ? 'docker.exe' : 'docker'

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
  await ensureDocker()
  await buildArtifacts()
  await runCommand(dockerCommand, ['build', '-t', 'ade-web:local', 'apps/web'])
  await runCommand(dockerCommand, ['build', '-t', 'ade-api:local', 'apps/api'])
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
