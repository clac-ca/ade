import * as assert from 'node:assert'
import { test } from 'node:test'
import { readConfig } from '../src/config'

test('readConfig returns defaults', () => {
  const config = readConfig({})

  assert.equal(config.host, '127.0.0.1')
  assert.equal(config.port, 8001)
  assert.deepStrictEqual(config.buildInfo, {
    service: 'ade-api',
    version: 'dev',
    gitSha: 'dev',
    builtAt: 'unknown',
    nodeVersion: process.version
  })
})

test('readConfig rejects invalid ports', () => {
  assert.throws(
    () => readConfig({
      ADE_API_PORT: 'abc'
    }),
    /ADE_API_PORT must be a positive integer/
  )
})
