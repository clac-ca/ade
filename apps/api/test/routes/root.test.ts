import { test } from 'node:test'
import * as assert from 'node:assert'
import { build } from '../helper'

test('default root route', async (t) => {
  const { app } = await build(t)

  const res = await app.inject({
    url: '/'
  })
  assert.deepStrictEqual(JSON.parse(res.payload), {
    service: 'ade-api',
    status: 'ok',
    version: 'test-version'
  })
})

test('health route', async (t) => {
  const { app } = await build(t)

  const res = await app.inject({
    url: '/healthz'
  })
  assert.equal(res.statusCode, 200)
  assert.deepStrictEqual(JSON.parse(res.payload), {
    service: 'ade-api',
    status: 'ok'
  })
})

test('ready route reflects readiness state', async (t) => {
  const { app, readiness } = await build(t, {
    ready: false
  })

  const notReady = await app.inject({
    url: '/readyz'
  })
  assert.equal(notReady.statusCode, 503)
  assert.deepStrictEqual(JSON.parse(notReady.payload), {
    service: 'ade-api',
    status: 'not-ready'
  })

  readiness.isReady = true

  const ready = await app.inject({
    url: '/readyz'
  })
  assert.equal(ready.statusCode, 200)
  assert.deepStrictEqual(JSON.parse(ready.payload), {
    service: 'ade-api',
    status: 'ready'
  })
})

test('version route exposes build metadata', async (t) => {
  const { app, buildInfo } = await build(t)

  const res = await app.inject({
    url: '/version'
  })

  assert.equal(res.statusCode, 200)
  assert.deepStrictEqual(JSON.parse(res.payload), {
    ...buildInfo,
    nodeVersion: process.version
  })
})
