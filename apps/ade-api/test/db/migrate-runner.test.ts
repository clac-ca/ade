import test from 'node:test'
import assert from 'node:assert/strict'
import { splitSqlBatches } from '../../src/db/migrate-runner'

test('splitSqlBatches returns a single batch when no GO separators are present', () => {
  assert.deepEqual(splitSqlBatches('SELECT 1;'), ['SELECT 1;'])
})

test('splitSqlBatches splits on standalone GO separators', () => {
  assert.deepEqual(
    splitSqlBatches(`
      CREATE TABLE dbo.example (id INT NOT NULL);
      GO

      INSERT INTO dbo.example (id) VALUES (1);
      go
    `),
    [
      'CREATE TABLE dbo.example (id INT NOT NULL);',
      'INSERT INTO dbo.example (id) VALUES (1);'
    ]
  )
})

test('splitSqlBatches does not split on GO inside other tokens', () => {
  assert.deepEqual(
    splitSqlBatches(`
      SELECT 'go';
      SELECT N'Gopher';
    `),
    [
      "SELECT 'go';\n      SELECT N'Gopher';"
    ]
  )
})
