type ReadinessPhase = "degraded" | "ready" | "starting" | "stopping";

type ReadinessSnapshot = {
  database: {
    lastCheckedAt: number | null;
    lastError: string | null;
    ok: boolean;
    staleAfterMs: number;
  };
  phase: ReadinessPhase;
};

type CreateReadinessControllerOptions = {
  databaseOk?: boolean;
  lastCheckedAt?: number | null;
  lastError?: string | null;
  phase?: ReadinessPhase;
  staleAfterMs?: number;
};

type ReadinessController = {
  markDegraded(error?: unknown): void;
  markReady(): void;
  markStarting(): void;
  markStopping(): void;
  recordDatabaseFailure(error: unknown): void;
  recordDatabaseSuccess(): void;
  snapshot(): ReadinessSnapshot;
};

function createReadinessController(
  options: CreateReadinessControllerOptions = {},
): ReadinessController {
  let state: ReadinessSnapshot = {
    database: {
      lastCheckedAt: options.lastCheckedAt ?? null,
      lastError: options.lastError ?? null,
      ok: options.databaseOk ?? false,
      staleAfterMs: options.staleAfterMs ?? 15_000,
    },
    phase: options.phase ?? "starting",
  };

  function update(nextState: ReadinessSnapshot) {
    state = nextState;
  }

  return {
    markDegraded(error?: unknown) {
      if (state.phase === "stopping") {
        return;
      }

      update({
        ...state,
        database: {
          ...state.database,
          lastError:
            error instanceof Error
              ? error.message
              : typeof error === "string"
                ? error
                : null,
          ok: false,
        },
        phase: "degraded",
      });
    },
    markReady() {
      if (state.phase === "stopping") {
        return;
      }

      update({
        ...state,
        phase: "ready",
      });
    },
    markStarting() {
      update({
        ...state,
        phase: "starting",
      });
    },
    markStopping() {
      update({
        ...state,
        phase: "stopping",
      });
    },
    recordDatabaseFailure(error: unknown) {
      update({
        ...state,
        database: {
          ...state.database,
          lastCheckedAt: Date.now(),
          lastError:
            error instanceof Error
              ? error.message
              : typeof error === "string"
                ? error
                : null,
          ok: false,
        },
      });
    },
    recordDatabaseSuccess() {
      update({
        ...state,
        database: {
          ...state.database,
          lastCheckedAt: Date.now(),
          lastError: null,
          ok: true,
        },
      });
    },
    snapshot() {
      return {
        database: {
          ...state.database,
        },
        phase: state.phase,
      };
    },
  };
}

function isReadinessStale(
  readiness: ReadinessSnapshot,
  now = Date.now(),
): boolean {
  if (readiness.database.lastCheckedAt === null) {
    return true;
  }

  return (
    now - readiness.database.lastCheckedAt > readiness.database.staleAfterMs
  );
}

function isApplicationReady(
  readiness: ReadinessSnapshot,
  now = Date.now(),
): boolean {
  return (
    readiness.phase === "ready" &&
    readiness.database.ok &&
    !isReadinessStale(readiness, now)
  );
}

export { createReadinessController, isApplicationReady, isReadinessStale };

export type {
  CreateReadinessControllerOptions,
  ReadinessController,
  ReadinessPhase,
  ReadinessSnapshot,
};
