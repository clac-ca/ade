import { test } from 'node:test'
import * as assert from 'node:assert'
import { build } from '../helper'

test('default root route', async (t) => {
  const app = await build(t)

  const res = await app.inject({
    url: '/'
  })
  assert.deepStrictEqual(JSON.parse(res.payload), {
    service: 'ade-api',
    status: 'ok'
  })
})

test('health route', async (t) => {
  const app = await build(t)

  const res = await app.inject({
    url: '/healthz'
  })
  assert.equal(res.statusCode, 200)
  assert.deepStrictEqual(JSON.parse(res.payload), {
    service: 'ade-api',
    status: 'ok'
  })
})
