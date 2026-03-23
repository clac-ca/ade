import { mkdtempSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import * as assert from 'node:assert'
import { test } from 'node:test'
import { readConfig } from '../src/config'

test('readConfig returns development defaults when bundled build info is absent', () => {
  const config = readConfig({}, {
    buildInfoPath: join(tmpdir(), 'missing-build-info.json')
  })

  assert.equal(config.host, '127.0.0.1')
  assert.equal(config.port, 8000)
  assert.equal(config.buildInfo.service, 'ade')
  assert.match(config.buildInfo.version, /\S+/)
  assert.match(config.buildInfo.gitSha, /\S+/)
  assert.match(config.buildInfo.builtAt, /\S+/)
})

test('readConfig rejects invalid ports', () => {
  for (const value of ['', '-1', '0', '1e3', '123abc', 'abc']) {
    assert.throws(
      () => readConfig({
        PORT: value
      }),
      /PORT/
    )
  }
})

test('readConfig ignores runtime provenance overrides when bundled build info exists', () => {
  const tempDir = mkdtempSync(join(tmpdir(), 'ade-build-info-'))
  const buildInfoPath = join(tempDir, 'build-info.json')

  writeFileSync(buildInfoPath, JSON.stringify({
    builtAt: '2026-03-21T00:00:00.000Z',
    gitSha: 'sha-from-file',
    service: 'ade',
    version: 'version-from-file'
  }))

  const config = readConfig({
    ADE_BUILD_GIT_SHA: 'override',
    ADE_BUILD_TIMESTAMP: 'override',
    ADE_BUILD_VERSION: 'override'
  }, {
    buildInfoPath
  })

  assert.deepStrictEqual(config.buildInfo, {
    builtAt: '2026-03-21T00:00:00.000Z',
    gitSha: 'sha-from-file',
    service: 'ade',
    version: 'version-from-file'
  })
})

test('readConfig rejects missing bundled build info in production', () => {
  assert.throws(
    () => readConfig({
      NODE_ENV: 'production'
    }, {
      buildInfoPath: join(tmpdir(), 'missing-build-info.json')
    }),
    /Missing ADE build info/
  )
})

test('readConfig rejects invalid bundled build info', () => {
  const tempDir = mkdtempSync(join(tmpdir(), 'ade-build-info-invalid-'))
  const buildInfoPath = join(tempDir, 'build-info.json')

  writeFileSync(buildInfoPath, JSON.stringify({
    builtAt: '',
    gitSha: 'sha',
    service: 'ade',
    version: '0.1.0'
  }))

  assert.throws(
    () => readConfig({}, {
      buildInfoPath
    }),
    /non-empty string/
  )
})
