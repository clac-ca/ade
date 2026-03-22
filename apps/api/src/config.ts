import process from 'node:process'

export type BuildInfo = {
  service: 'ade-api',
  version: string,
  gitSha: string,
  builtAt: string,
  nodeVersion: string
}

export type ApiConfig = {
  host: string,
  port: number,
  buildInfo: BuildInfo
}

function readPort(value: string | undefined): number {
  const rawValue = value ?? '8001'
  const port = Number.parseInt(rawValue, 10)

  if (!Number.isInteger(port) || port <= 0) {
    throw new Error(`ADE_API_PORT must be a positive integer, received: ${rawValue}`)
  }

  return port
}

function readBuildInfo(env: NodeJS.ProcessEnv): BuildInfo {
  return {
    service: 'ade-api',
    version: env.ADE_BUILD_VERSION ?? 'dev',
    gitSha: env.ADE_BUILD_GIT_SHA ?? 'dev',
    builtAt: env.ADE_BUILD_TIMESTAMP ?? 'unknown',
    nodeVersion: process.version
  }
}

function readConfig(env: NodeJS.ProcessEnv = process.env): ApiConfig {
  return {
    host: env.ADE_API_HOST?.trim() || '127.0.0.1',
    port: readPort(env.ADE_API_PORT),
    buildInfo: readBuildInfo(env)
  }
}

export {
  readConfig
}
