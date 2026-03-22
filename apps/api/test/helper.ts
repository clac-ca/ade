import * as test from 'node:test'
import { createApp } from '../src/app'
import { BuildInfo } from '../src/config'

export type TestContext = {
  after: typeof test.after
}

export type BuildOptions = {
  buildInfo?: BuildInfo,
  ready?: boolean
}

const defaultBuildInfo: BuildInfo = {
  service: 'ade-api',
  version: 'test-version',
  gitSha: 'test-git-sha',
  builtAt: '2026-03-21T00:00:00.000Z',
  nodeVersion: 'v22.0.0'
}

async function build(t: TestContext, options: BuildOptions = {}) {
  const readiness = {
    isReady: options.ready ?? true
  }
  const fastify = createApp({
    buildInfo: options.buildInfo ?? defaultBuildInfo,
    logger: false,
    readiness
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
