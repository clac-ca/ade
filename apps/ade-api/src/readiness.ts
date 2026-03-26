export type ReadinessState = {
  isStarted: boolean,
  database: {
    ok: boolean,
    lastCheckedAt: number | null,
    lastError: string | null,
    staleAfterMs: number
  }
}

export type CreateReadinessStateOptions = {
  databaseOk?: boolean,
  isStarted?: boolean,
  lastCheckedAt?: number | null,
  lastError?: string | null,
  staleAfterMs?: number
}

function createReadinessState(options: CreateReadinessStateOptions = {}): ReadinessState {
  return {
    isStarted: options.isStarted ?? false,
    database: {
      ok: options.databaseOk ?? false,
      lastCheckedAt: options.lastCheckedAt ?? null,
      lastError: options.lastError ?? null,
      staleAfterMs: options.staleAfterMs ?? 15_000
    }
  }
}

function isReadinessStale(readiness: ReadinessState, now = Date.now()): boolean {
  if (readiness.database.lastCheckedAt === null) {
    return true
  }

  return now - readiness.database.lastCheckedAt > readiness.database.staleAfterMs
}

function isApplicationReady(readiness: ReadinessState, now = Date.now()): boolean {
  if (!readiness.isStarted) {
    return false
  }

  if (!readiness.database.ok) {
    return false
  }

  return !isReadinessStale(readiness, now)
}

export {
  createReadinessState,
  isApplicationReady,
  isReadinessStale
}
