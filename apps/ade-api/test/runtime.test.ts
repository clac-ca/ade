import * as assert from 'node:assert'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { createRuntime } from '../src/runtime'

const webRoot = fileURLToPath(new URL('./fixtures/web-dist', import.meta.url))
const buildInfo = {
  builtAt: '2026-03-21T00:00:00.000Z',
  gitSha: 'test-git-sha',
  service: 'ade' as const,
  version: 'test-version'
}

test('runtime toggles readiness during start and stop', async (t) => {
  const runtime = createRuntime({
    buildInfo,
    host: '127.0.0.1',
    logger: false,
    port: 0,
    webRoot
  })

  t.after(async () => {
    if (runtime.app.server.listening) {
      await runtime.stop()
    }
  })

  await runtime.app.ready()

  const beforeStart = await runtime.app.inject({
    url: '/api/readyz'
  })
  assert.equal(beforeStart.statusCode, 503)
  assert.equal(runtime.readiness.isReady, false)

  await runtime.start()
  assert.equal(runtime.readiness.isReady, true)

  const address = runtime.app.server.address()
  assert.ok(address && typeof address !== 'string')

  const afterStart = await fetch(`http://127.0.0.1:${address.port}/api/readyz`)
  assert.equal(afterStart.status, 200)

  await runtime.stop()
  assert.equal(runtime.readiness.isReady, false)
})
