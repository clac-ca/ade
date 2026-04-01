use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{
    net::TcpListener,
    sync::Notify,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tower::make::Shared;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::{
    api::{AppState, create_app},
    db::DatabaseProbe,
    error::AppError,
    readiness::{DatabaseReadiness, ReadinessController, ReadinessPhase, ReadinessSnapshot},
    runs::RunService,
    scope_session::ScopeSessionService,
    terminal::TerminalService,
};

pub struct ServerOptions {
    pub host: String,
    pub port: u16,
    pub probe_interval_ms: u64,
    pub scope_session_service: Arc<ScopeSessionService>,
    pub run_service: Arc<RunService>,
    pub terminal_service: Arc<TerminalService>,
    pub stale_after_ms: u64,
    pub web_root: Option<PathBuf>,
    pub database: Arc<dyn DatabaseProbe>,
}

pub struct ServerInstance {
    app: axum::Router,
    readiness: ReadinessController,
    database: Arc<dyn DatabaseProbe>,
    host: String,
    port: u16,
    probe_interval_ms: u64,
    server_task: Option<JoinHandle<Result<(), std::io::Error>>>,
    shutdown: Option<Arc<Notify>>,
    probe_task: Option<JoinHandle<()>>,
}

impl ServerInstance {
    #[must_use]
    pub fn new(options: ServerOptions) -> Self {
        let readiness = ReadinessController::new(ReadinessSnapshot {
            database: DatabaseReadiness {
                stale_after_ms: options.stale_after_ms,
                ..DatabaseReadiness::default()
            },
            ..ReadinessSnapshot::default()
        });
        let app = create_app(AppState {
            readiness: readiness.clone(),
            scope_session_service: options.scope_session_service,
            run_service: options.run_service,
            terminal_service: options.terminal_service,
            web_root: options.web_root,
        });

        Self {
            app,
            readiness,
            database: options.database,
            host: options.host,
            port: options.port,
            probe_interval_ms: options.probe_interval_ms,
            server_task: None,
            shutdown: None,
            probe_task: None,
        }
    }

    pub async fn start(&mut self) -> Result<(), AppError> {
        self.readiness.mark_starting();
        let database = Arc::clone(&self.database);

        match database.ping().await {
            Ok(()) => {
                self.readiness.record_database_success(unix_time_ms());
                self.readiness.mark_ready();
            }
            Err(error) => {
                self.readiness
                    .record_database_failure(unix_time_ms(), Some(&error.to_string()));
                self.readiness.mark_degraded(Some(&error.to_string()));
                return Err(AppError::startup_with_source(
                    "Failed to verify SQL connectivity during startup.",
                    error,
                ));
            }
        }

        let listener_host: IpAddr = self
            .host
            .parse()
            .map_err(|error| AppError::config_with_source("Invalid listen host.", error))?;
        let listener = TcpListener::bind(SocketAddr::from((listener_host, self.port)))
            .await
            .map_err(|error| {
                AppError::startup_with_source("Failed to bind the ADE API server.", error)
            })?;

        let shutdown = Arc::new(Notify::new());
        let server = axum::serve(listener, Shared::new(self.app.clone())).with_graceful_shutdown({
            let shutdown = Arc::clone(&shutdown);
            async move {
                shutdown.notified().await;
            }
        });
        let readiness = self.readiness.clone();
        let database_for_probe = Arc::clone(&database);
        let probe_interval_ms = self.probe_interval_ms;
        let probe_shutdown = shutdown.clone();

        self.server_task = Some(tokio::spawn(async move { server.await }));
        self.probe_task = Some(tokio::spawn(async move {
            run_probe_loop(
                readiness,
                database_for_probe,
                probe_shutdown,
                probe_interval_ms,
            )
            .await;
        }));
        self.shutdown = Some(shutdown);

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), AppError> {
        self.readiness.mark_stopping();

        if let Some(shutdown) = self.shutdown.take() {
            shutdown.notify_waiters();
        }

        if let Some(probe_task) = self.probe_task.take() {
            probe_task.await.map_err(|error| {
                AppError::internal_with_source("Probe task failed to stop cleanly.", error)
            })?;
        }

        if let Some(server_task) = self.server_task.take() {
            server_task
                .await
                .map_err(|error| {
                    AppError::internal_with_source("Server task failed to stop cleanly.", error)
                })?
                .map_err(|error| {
                    AppError::internal_with_source("Axum server exited with an IO error.", error)
                })?;
        }

        self.database.close().await?;

        Ok(())
    }
}

async fn run_probe_loop(
    readiness: ReadinessController,
    database: Arc<dyn DatabaseProbe>,
    shutdown: Arc<Notify>,
    probe_interval_ms: u64,
) {
    let mut ticker = interval(Duration::from_millis(probe_interval_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown.notified() => return,
            _ = ticker.tick() => {
                let previous_phase = readiness.snapshot().phase;

                match database.ping().await {
                    Ok(()) => {
                        readiness.record_database_success(unix_time_ms());
                        readiness.mark_ready();

                        if previous_phase == ReadinessPhase::Degraded {
                            info!("SQL readiness probe recovered.");
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        readiness.record_database_failure(unix_time_ms(), Some(&message));
                        readiness.mark_degraded(Some(&message));
                        error!(error = %message, "SQL readiness probe failed.");
                    }
                }
            }
        }
    }
}

#[must_use]
pub fn unix_time_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

pub fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .try_init();
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use tempfile::tempdir;
    use tokio::time::sleep;

    use super::*;
    use crate::{
        config::DEFAULT_READINESS_STALE_AFTER_MS,
        runs::{InMemoryRunStore, RunService},
        scope_session::ScopeSessionService,
        terminal::TerminalService,
    };

    #[derive(Debug, Default)]
    struct FakeDatabase {
        outcomes: Mutex<Vec<Result<(), String>>>,
    }

    impl FakeDatabase {
        fn new(outcomes: Vec<Result<(), String>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes),
            }
        }
    }

    #[async_trait]
    impl DatabaseProbe for FakeDatabase {
        async fn ping(&self) -> Result<(), AppError> {
            let mut outcomes = self.outcomes.lock().expect("outcomes lock poisoned");

            match outcomes.first().cloned().unwrap_or(Ok(())) {
                Ok(()) => {
                    if outcomes.len() > 1 {
                        let _ = outcomes.remove(0);
                    }
                    Ok(())
                }
                Err(message) => {
                    if outcomes.len() > 1 {
                        let _ = outcomes.remove(0);
                    }
                    Err(AppError::database(message))
                }
            }
        }

        async fn close(&self) -> Result<(), AppError> {
            Ok(())
        }
    }

    fn fixture_scope_session_service() -> Arc<ScopeSessionService> {
        let tempdir = tempdir().unwrap();
        let bundle_root = tempdir.path().join("session-bundle");
        let config_root = tempdir.path().join("session-configs");
        fs::create_dir_all(bundle_root.join("bin")).unwrap();
        fs::create_dir_all(bundle_root.join("python")).unwrap();
        fs::create_dir_all(bundle_root.join("wheelhouse/base")).unwrap();
        fs::create_dir_all(config_root.join("workspace-a/config-v1")).unwrap();
        let connector = bundle_root.join("bin/reverse-connect");
        let prepare = bundle_root.join("bin/prepare.sh");
        let run_script = bundle_root.join("bin/run.py");
        let engine = bundle_root.join("wheelhouse/base/ade_engine-0.1.0-py3-none-any.whl");
        let config = config_root.join("workspace-a/config-v1/ade_config-0.1.0-py3-none-any.whl");
        let toolchain = bundle_root.join("python/python-3.12.11-linux-x86_64.tar.gz");
        fs::write(&connector, b"connector").unwrap();
        fs::write(&prepare, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::write(&run_script, b"print('ok')\n").unwrap();
        fs::write(&engine, b"engine").unwrap();
        fs::write(&config, b"config").unwrap();
        fs::write(&toolchain, b"toolchain").unwrap();
        std::mem::forget(tempdir);

        Arc::new(
            ScopeSessionService::from_paths(
                "http://127.0.0.1:8000",
                "http://127.0.0.1:9",
                "test-session-secret",
                bundle_root,
                config_root,
            )
            .unwrap(),
        )
    }

    fn fixture_terminal_service(
        scope_session_service: Arc<ScopeSessionService>,
    ) -> Arc<TerminalService> {
        let env = [(
            "ADE_PUBLIC_API_URL".to_string(),
            "http://127.0.0.1:8000".to_string(),
        )]
        .into_iter()
        .collect();
        Arc::new(TerminalService::from_env(&env, scope_session_service).unwrap())
    }

    fn fixture_run_service(scope_session_service: Arc<ScopeSessionService>) -> Arc<RunService> {
        let env = [
            (
                "ADE_PUBLIC_API_URL".to_string(),
                "http://127.0.0.1:8000".to_string(),
            ),
            (
                "ADE_BLOB_ACCOUNT_URL".to_string(),
                "http://127.0.0.1:65535/devstoreaccount1".to_string(),
            ),
            (
                "ADE_BLOB_CONTAINER".to_string(),
                "documents".to_string(),
            ),
            (
                "ADE_BLOB_ACCOUNT_KEY".to_string(),
                "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=="
                    .to_string(),
            ),
        ]
        .into_iter()
        .collect();
        Arc::new(
            RunService::from_env(
                &env,
                scope_session_service,
                Arc::new(InMemoryRunStore::default()),
            )
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn startup_fails_when_initial_probe_fails() {
        let database: Arc<dyn DatabaseProbe> =
            Arc::new(FakeDatabase::new(vec![Err("sql unavailable".to_string())]));
        let scope_session_service = fixture_scope_session_service();
        let mut server = ServerInstance::new(ServerOptions {
            host: "127.0.0.1".to_string(),
            port: 0,
            probe_interval_ms: 10,
            scope_session_service: Arc::clone(&scope_session_service),
            run_service: fixture_run_service(Arc::clone(&scope_session_service)),
            terminal_service: fixture_terminal_service(Arc::clone(&scope_session_service)),
            stale_after_ms: DEFAULT_READINESS_STALE_AFTER_MS,
            web_root: None,
            database,
        });

        let error = server.start().await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "Failed to verify SQL connectivity during startup."
        );
    }

    #[tokio::test]
    async fn readiness_degrades_and_recovers() {
        let database: Arc<dyn DatabaseProbe> = Arc::new(FakeDatabase::new(vec![
            Ok(()),
            Err("sql temporarily unavailable".to_string()),
            Ok(()),
            Ok(()),
        ]));
        let scope_session_service = fixture_scope_session_service();
        let mut server = ServerInstance::new(ServerOptions {
            host: "127.0.0.1".to_string(),
            port: 0,
            probe_interval_ms: 10,
            scope_session_service: Arc::clone(&scope_session_service),
            run_service: fixture_run_service(Arc::clone(&scope_session_service)),
            terminal_service: fixture_terminal_service(Arc::clone(&scope_session_service)),
            stale_after_ms: DEFAULT_READINESS_STALE_AFTER_MS,
            web_root: None,
            database,
        });

        server.start().await.unwrap();
        wait_for_phase(&server.readiness, ReadinessPhase::Degraded).await;
        wait_for_phase(&server.readiness, ReadinessPhase::Ready).await;
        server.stop().await.unwrap();
    }

    async fn wait_for_phase(readiness: &ReadinessController, expected: ReadinessPhase) {
        for _ in 0..40 {
            if readiness.snapshot().phase == expected {
                return;
            }

            sleep(Duration::from_millis(5)).await;
        }

        panic!(
            "expected readiness phase {:?}, received {:?}",
            expected,
            readiness.snapshot().phase
        );
    }
}
