import * as assert from 'node:assert'
import { test } from 'node:test'
import { runServer } from '../src/server'

test('runServer exits non-zero when shutdown fails', async (t) => {
  let signalHandler: (() => void) | undefined
  const exitCodes: number[] = []
  const originalConsoleError = console.error
  const processHandle = {
    exit(code: number) {
      exitCodes.push(code)
    },
    on(event: string, handler: () => void) {
      if (event === 'SIGTERM') {
        signalHandler = handler
      }
    }
  }
  console.error = () => {}
  t.after(() => {
    console.error = originalConsoleError
  })

  const runtime = {
    start: async () => {},
    stop: async () => {
      throw new Error('close failed')
    }
  }

  await runServer(processHandle, runtime)
  assert.ok(signalHandler)

  await signalHandler?.()
  assert.deepStrictEqual(exitCodes, [1])
})

test('runServer exits non-zero when startup fails', async (t) => {
  const exitCodes: number[] = []
  const originalConsoleError = console.error
  const processHandle = {
    exit(code: number) {
      exitCodes.push(code)
    },
    on() {}
  }
  console.error = () => {}
  t.after(() => {
    console.error = originalConsoleError
  })

  const runtime = {
    start: async () => {
      throw new Error('sql unavailable')
    },
    stop: async () => {}
  }

  await runServer(processHandle, runtime)
  assert.deepStrictEqual(exitCodes, [1])
})
