import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import process from 'node:process'

export type BundledBuildInfo = {
  service: 'ade',
  version: string,
  gitSha: string,
  builtAt: string
}

export type VersionInfo = BundledBuildInfo & {
  nodeVersion: string
}

export type ApiConfig = {
  host: string,
  port: number,
  buildInfo: BundledBuildInfo,
  sqlConnectionString?: string,
  blobStorage: {
    connectionString?: string,
    resourceEndpoint?: string
  }
}

export type ReadConfigOptions = {
  buildInfoPath?: string,
  requireSql?: boolean
}

const packagePath = join(__dirname, '..', 'package.json')
const bundledBuildInfoPath = join(__dirname, 'build-info.json')

function readPort(value: string | undefined): number {
  if (value === undefined) {
    return 8000
  }

  const rawValue = value.trim()

  if (!/^[1-9]\d*$/.test(rawValue)) {
    throw new Error(`PORT must be a positive integer, received: ${rawValue}`)
  }

  const port = Number.parseInt(rawValue, 10)

  if (port > 65_535) {
    throw new Error(`PORT must be 65535 or lower, received: ${rawValue}`)
  }

  return port
}

function readDevelopmentBuildInfo(): BundledBuildInfo {
  const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'))

  return {
    builtAt: readGitValue(['show', '--no-patch', '--format=%cI', 'HEAD']) ?? 'dev',
    gitSha: readGitValue(['rev-parse', 'HEAD']) ?? 'dev',
    service: 'ade',
    version: packageJson.version
  }
}

function readGitValue(args: string[]) {
  try {
    return execFileSync('git', args, {
      cwd: join(__dirname, '..'),
      encoding: 'utf8'
    }).trim()
  } catch {
    return null
  }
}

function validateBuildInfo(value: unknown): BundledBuildInfo {
  if (!value || typeof value !== 'object') {
    throw new Error('ADE build info must be an object.')
  }

  const buildInfo = value as Record<string, unknown>

  for (const key of ['service', 'version', 'gitSha', 'builtAt']) {
    const field = buildInfo[key]

    if (typeof field !== 'string' || field.trim() === '') {
      throw new Error(`ADE build info field "${key}" must be a non-empty string.`)
    }
  }

  if (buildInfo.service !== 'ade') {
    throw new Error('ADE build info service must be "ade".')
  }

  const builtAt = buildInfo.builtAt as string
  const gitSha = buildInfo.gitSha as string
  const version = buildInfo.version as string

  return {
    builtAt,
    gitSha,
    service: 'ade',
    version
  }
}

function readBuildInfo(env: NodeJS.ProcessEnv, options: ReadConfigOptions): BundledBuildInfo {
  const buildInfoPath = options.buildInfoPath ?? bundledBuildInfoPath

  if (existsSync(buildInfoPath)) {
    return validateBuildInfo(JSON.parse(readFileSync(buildInfoPath, 'utf8')))
  }

  if (env.NODE_ENV === 'production') {
    throw new Error(`Missing ADE build info at ${buildInfoPath}.`)
  }

  return readDevelopmentBuildInfo()
}

function readOptionalTrimmed(env: NodeJS.ProcessEnv, name: string): string | undefined {
  const value = env[name]

  if (value === undefined) {
    return undefined
  }

  const trimmed = value.trim()
  return trimmed === '' ? undefined : trimmed
}

function readRequiredTrimmed(env: NodeJS.ProcessEnv, name: string): string {
  const value = readOptionalTrimmed(env, name)

  if (value === undefined) {
    throw new Error(`Missing required environment variable: ${name}`)
  }

  return value
}

function readBlobStorage(env: NodeJS.ProcessEnv) {
  const connectionString = readOptionalTrimmed(env, 'AZURE_STORAGEBLOB_CONNECTIONSTRING')
  const resourceEndpoint = readOptionalTrimmed(env, 'AZURE_STORAGEBLOB_RESOURCEENDPOINT')

  if (connectionString && resourceEndpoint) {
    throw new Error(
      'Set either AZURE_STORAGEBLOB_CONNECTIONSTRING or AZURE_STORAGEBLOB_RESOURCEENDPOINT, not both.'
    )
  }

  if (resourceEndpoint) {
    const url = new URL(resourceEndpoint)

    if (url.protocol !== 'https:') {
      throw new Error('AZURE_STORAGEBLOB_RESOURCEENDPOINT must use https.')
    }
  }

  return {
    connectionString,
    resourceEndpoint
  }
}

function readConfig(env: NodeJS.ProcessEnv = process.env, options: ReadConfigOptions = {}): ApiConfig {
  return {
    host: env.HOST?.trim() || (env.NODE_ENV === 'production' ? '0.0.0.0' : '127.0.0.1'),
    port: readPort(env.PORT),
    buildInfo: readBuildInfo(env, options),
    sqlConnectionString: options.requireSql
      ? readRequiredTrimmed(env, 'AZURE_SQL_CONNECTIONSTRING')
      : readOptionalTrimmed(env, 'AZURE_SQL_CONNECTIONSTRING'),
    blobStorage: readBlobStorage(env)
  }
}

export {
  readConfig
}
