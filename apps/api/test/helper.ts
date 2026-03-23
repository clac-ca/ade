import { fileURLToPath } from 'node:url'
import * as test from 'node:test'
import { createApp } from '../src/app'
import { BundledBuildInfo } from '../src/config'

export type TestContext = {
  after: typeof test.after
}

export type BuildOptions = {
  buildInfo?: BundledBuildInfo,
  ready?: boolean
}

const defaultBuildInfo: BundledBuildInfo = {
  service: 'ade',
  version: 'test-version',
  gitSha: 'test-git-sha',
  builtAt: '2026-03-21T00:00:00.000Z'
}
const webRoot = fileURLToPath(new URL('./fixtures/web-dist', import.meta.url))

async function build(t: TestContext, options: BuildOptions = {}) {
  const readiness = {
    isReady: options.ready ?? true
  }
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
