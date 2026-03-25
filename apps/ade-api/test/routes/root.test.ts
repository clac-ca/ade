import { test } from 'node:test'
import * as assert from 'node:assert'
import { build } from '../helper'

test('default root route', async (t) => {
  const { app } = await build(t)

  const res = await app.inject({
    url: '/'
  })
  assert.equal(res.statusCode, 200)
  assert.match(res.headers['content-type'] ?? '', /text\/html/)
  assert.match(res.payload, /id="root"/)
})

test('health route', async (t) => {
  const { app } = await build(t)

  const res = await app.inject({
    url: '/api/healthz'
  })
  assert.equal(res.statusCode, 200)
  assert.deepStrictEqual(JSON.parse(res.payload), {
    service: 'ade',
    status: 'ok'
  })
})

test('api root works without a trailing slash', async (t) => {
  const { app } = await build(t)

  const res = await app.inject({
    url: '/api'
  })

  assert.equal(res.statusCode, 200)
  assert.deepStrictEqual(JSON.parse(res.payload), {
    service: 'ade',
    status: 'ok',
    version: 'test-version'
  })
})

test('ready route reflects readiness state', async (t) => {
  const { app, readiness } = await build(t, {
    ready: false
  })

  const notReady = await app.inject({
    url: '/api/readyz'
  })
  assert.equal(notReady.statusCode, 503)
  assert.deepStrictEqual(JSON.parse(notReady.payload), {
    service: 'ade',
    status: 'not-ready'
  })

  readiness.isReady = true

  const ready = await app.inject({
    url: '/api/readyz'
  })
  assert.equal(ready.statusCode, 200)
  assert.deepStrictEqual(JSON.parse(ready.payload), {
    service: 'ade',
    status: 'ready'
  })
})

test('version route exposes build metadata', async (t) => {
  const { app, buildInfo } = await build(t)

  const res = await app.inject({
    url: '/api/version'
  })

  assert.equal(res.statusCode, 200)
  assert.deepStrictEqual(JSON.parse(res.payload), {
    ...buildInfo,
    nodeVersion: process.version
  })
})

test('spa fallback serves index html for unknown frontend routes', async (t) => {
  const { app } = await build(t)

  const res = await app.inject({
    url: '/documents/example'
  })

  assert.equal(res.statusCode, 200)
  assert.match(res.headers['content-type'] ?? '', /text\/html/)
  assert.match(res.payload, /id="root"/)
})

test('unknown api routes still return 404', async (t) => {
  const { app } = await build(t)

  const res = await app.inject({
    url: '/api/unknown'
  })

  assert.equal(res.statusCode, 404)
})
