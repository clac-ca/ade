import process from 'node:process'
import { readConfig } from './config'
import { createRuntime, Runtime } from './runtime'

type ProcessHandle = {
  exit: (code: number) => void,
  on: (event: 'SIGINT' | 'SIGTERM', handler: () => void) => void
}
type ServerRuntime = Pick<Runtime, 'start' | 'stop'>

function createProductionRuntime(): Runtime {
  const config = readConfig()

  return createRuntime({
    buildInfo: config.buildInfo,
    host: config.host,
    port: config.port
  })
}

async function runServer(processHandle: ProcessHandle = process, runtime: ServerRuntime = createProductionRuntime()) {
  let shuttingDown = false

  async function stop(exitCode: number) {
    if (shuttingDown) {
      return
    }

    shuttingDown = true

    try {
      await runtime.stop()
      processHandle.exit(exitCode)
    } catch (error) {
      console.error(error)
      processHandle.exit(1)
    }
  }

  processHandle.on('SIGINT', () => {
    void stop(0)
  })

  processHandle.on('SIGTERM', () => {
    void stop(0)
  })

  try {
    await runtime.start()
  } catch (error) {
    console.error(error)
    await stop(1)
  }
}

if (require.main === module) {
  void runServer().catch((error) => {
    console.error(error)
    process.exit(1)
  })
}

export {
  runServer
}
