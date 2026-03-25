import * as assert from 'node:assert'
import { test } from 'node:test'
import {
  buildPoolConfig,
  parseSqlConnectionString,
  quoteSqlIdentifier,
  withDatabase
} from '../../src/db/connection'

test('parseSqlConnectionString supports SQL auth connections', () => {
  const parsed = parseSqlConnectionString(
    'Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234;Encrypt=false;TrustServerCertificate=true'
  )

  assert.deepStrictEqual(parsed, {
    authentication: 'sql-password',
    database: 'ade',
    encrypt: false,
    password: 'Password!234',
    port: 1433,
    server: '127.0.0.1',
    trustServerCertificate: true,
    userId: 'sa'
  })
})

test('parseSqlConnectionString supports managed identity connections', () => {
  const parsed = parseSqlConnectionString(
    'Data Source=tcp:sql-ade.database.windows.net,1433;Initial Catalog=ade;User ID=11111111-1111-1111-1111-111111111111;Authentication=ActiveDirectoryManagedIdentity;Encrypt=true;TrustServerCertificate=false'
  )

  assert.deepStrictEqual(parsed, {
    authentication: 'managed-identity',
    database: 'ade',
    encrypt: true,
    password: undefined,
    port: 1433,
    server: 'sql-ade.database.windows.net',
    trustServerCertificate: false,
    userId: '11111111-1111-1111-1111-111111111111'
  })
})

test('buildPoolConfig translates managed identity connections into mssql config', () => {
  const config = buildPoolConfig(
    'Data Source=tcp:sql-ade.database.windows.net,1433;Initial Catalog=ade;Authentication=ActiveDirectoryManagedIdentity;Encrypt=true;TrustServerCertificate=false'
  )

  assert.equal(config.authentication?.type, 'azure-active-directory-default')
  assert.equal(config.database, 'ade')
  assert.equal(config.port, 1433)
  assert.equal(config.server, 'sql-ade.database.windows.net')
})

test('quoteSqlIdentifier escapes closing brackets', () => {
  assert.equal(quoteSqlIdentifier('ade]identity'), '[ade]]identity]')
})

test('withDatabase rewrites the database name', () => {
  assert.equal(
    withDatabase(
      'Server=127.0.0.1,1433;Database=ade;User Id=sa;Password=Password!234',
      'master'
    ),
    'server=127.0.0.1,1433;database=master;user id=sa;password=Password!234'
  )
})
