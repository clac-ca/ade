import { join } from 'node:path'
import * as test from 'node:test'
import { createApp } from '../src/app'
import { BundledBuildInfo } from '../src/config'
import { createReadinessState } from '../src/readiness'

export type TestContext = {
  after: typeof test.after
}

export type BuildOptions = {
  buildInfo?: BundledBuildInfo,
  databaseOk?: boolean,
  isStarted?: boolean,
  lastCheckedAt?: number | null,
  staleAfterMs?: number
}

const defaultBuildInfo: BundledBuildInfo = {
  service: 'ade',
  version: 'test-version',
  gitSha: 'test-git-sha',
  builtAt: '2026-03-21T00:00:00.000Z'
}
const webRoot = join(__dirname, 'fixtures', 'web-dist')

async function build(t: TestContext, options: BuildOptions = {}) {
  const readiness = createReadinessState({
    databaseOk: options.databaseOk ?? true,
    isStarted: options.isStarted ?? true,
    lastCheckedAt: options.lastCheckedAt ?? Date.now(),
    staleAfterMs: options.staleAfterMs
  })
  const fastify = createApp({
    buildInfo: options.buildInfo ?? defaultBuildInfo,
    logger: false,
    readiness,
    webRoot
  })

  await fastify.ready()

  t.after(() => void fastify.close())
  return {
    app: fastify,
    buildInfo: options.buildInfo ?? defaultBuildInfo,
    readiness
  }
}

export {
  build
}
