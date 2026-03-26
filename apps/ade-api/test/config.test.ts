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
  assert.equal(config.sqlConnectionString, undefined)
  assert.equal(config.blobStorage.connectionString, undefined)
  assert.equal(config.blobStorage.resourceEndpoint, undefined)
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

test('readConfig rejects missing SQL when required', () => {
  assert.throws(
    () => readConfig({}, {
      buildInfoPath: join(tmpdir(), 'missing-build-info.json'),
      requireSql: true
    }),
    /AZURE_SQL_CONNECTIONSTRING/
  )
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

test('readConfig reads optional SQL and Blob settings', () => {
  const config = readConfig({
    AZURE_SQL_CONNECTIONSTRING: 'Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true',
    AZURE_STORAGEBLOB_CONNECTIONSTRING: 'UseDevelopmentStorage=true'
  }, {
    buildInfoPath: join(tmpdir(), 'missing-build-info.json')
  })

  assert.match(config.sqlConnectionString ?? '', /Database=ade/)
  assert.equal(config.blobStorage.connectionString, 'UseDevelopmentStorage=true')
  assert.equal(config.blobStorage.resourceEndpoint, undefined)
})

test('readConfig rejects multiple Blob transport settings', () => {
  assert.throws(
    () => readConfig({
      AZURE_STORAGEBLOB_CONNECTIONSTRING: 'UseDevelopmentStorage=true',
      AZURE_STORAGEBLOB_RESOURCEENDPOINT: 'https://example.blob.core.windows.net/'
    }, {
      buildInfoPath: join(tmpdir(), 'missing-build-info.json')
    }),
    /not both/
  )
})

test('readConfig rejects insecure Blob resource endpoints', () => {
  assert.throws(
    () => readConfig({
      AZURE_STORAGEBLOB_RESOURCEENDPOINT: 'http://example.blob.core.windows.net/'
    }, {
      buildInfoPath: join(tmpdir(), 'missing-build-info.json')
    }),
    /https/
  )
})
