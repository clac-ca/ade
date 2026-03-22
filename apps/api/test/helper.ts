import * as test from 'node:test'
import Fastify from 'fastify'
import app from '../src/app'

export type TestContext = {
  after: typeof test.after
}

async function build (t: TestContext) {
  const fastify = Fastify()

  await fastify.register(app)
  await fastify.ready()

  t.after(() => void fastify.close())
  return fastify
}

export {
  build
}
