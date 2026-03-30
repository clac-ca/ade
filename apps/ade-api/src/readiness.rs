use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessPhase {
    Degraded,
    Ready,
    Starting,
    Stopping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseReadiness {
    pub last_checked_at: Option<u64>,
    pub last_error: Option<String>,
    pub ok: bool,
    pub stale_after_ms: u64,
}

impl Default for DatabaseReadiness {
    fn default() -> Self {
        Self {
            last_checked_at: None,
            last_error: None,
            ok: false,
            stale_after_ms: 15_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadinessSnapshot {
    pub database: DatabaseReadiness,
    pub phase: ReadinessPhase,
}

impl Default for ReadinessSnapshot {
    fn default() -> Self {
        Self {
            database: DatabaseReadiness::default(),
            phase: ReadinessPhase::Starting,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReadinessController {
    inner: Arc<Mutex<ReadinessSnapshot>>,
}

impl Default for ReadinessController {
    fn default() -> Self {
        Self::new(ReadinessSnapshot::default())
    }
}

impl ReadinessController {
    #[must_use]
    pub fn new(snapshot: ReadinessSnapshot) -> Self {
        Self {
            inner: Arc::new(Mutex::new(snapshot)),
        }
    }

    pub fn mark_degraded(&self, error: Option<&str>) {
        let mut state = self.inner.lock().expect("readiness lock poisoned");

        if state.phase == ReadinessPhase::Stopping {
            return;
        }

        state.database.last_error = error.map(ToOwned::to_owned);
        state.database.ok = false;
        state.phase = ReadinessPhase::Degraded;
    }

    pub fn mark_ready(&self) {
        let mut state = self.inner.lock().expect("readiness lock poisoned");

        if state.phase == ReadinessPhase::Stopping {
            return;
        }

        state.phase = ReadinessPhase::Ready;
    }

    pub fn mark_starting(&self) {
        let mut state = self.inner.lock().expect("readiness lock poisoned");
        state.phase = ReadinessPhase::Starting;
    }

    pub fn mark_stopping(&self) {
        let mut state = self.inner.lock().expect("readiness lock poisoned");
        state.phase = ReadinessPhase::Stopping;
    }

    pub fn record_database_failure(&self, checked_at: u64, error: Option<&str>) {
        let mut state = self.inner.lock().expect("readiness lock poisoned");
        state.database.last_checked_at = Some(checked_at);
        state.database.last_error = error.map(ToOwned::to_owned);
        state.database.ok = false;
    }

    pub fn record_database_success(&self, checked_at: u64) {
        let mut state = self.inner.lock().expect("readiness lock poisoned");
        state.database.last_checked_at = Some(checked_at);
        state.database.last_error = None;
        state.database.ok = true;
    }

    #[must_use]
    pub fn snapshot(&self) -> ReadinessSnapshot {
        self.inner.lock().expect("readiness lock poisoned").clone()
    }
}

#[must_use]
pub fn is_readiness_stale(readiness: &ReadinessSnapshot, now: u64) -> bool {
    readiness
        .database
        .last_checked_at
        .is_none_or(|last_checked_at| {
            now.saturating_sub(last_checked_at) > readiness.database.stale_after_ms
        })
}

#[must_use]
pub fn is_application_ready(readiness: &ReadinessSnapshot, now: u64) -> bool {
    readiness.phase == ReadinessPhase::Ready
        && readiness.database.ok
        && !is_readiness_stale(readiness, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_becomes_stale_when_no_probe_has_run() {
        let controller = ReadinessController::default();
        let readiness = controller.snapshot();

        assert!(is_readiness_stale(&readiness, 1));
        assert!(!is_application_ready(&readiness, 1));
    }
}
