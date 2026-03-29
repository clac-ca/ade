pub mod config;
pub mod db;
pub mod error;
pub mod readiness;
pub mod router;
pub mod routes;
pub mod session;
pub mod state;

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{
    net::TcpListener,
    signal,
    sync::watch,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tower::make::Shared;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::{
    config::DEFAULT_READINESS_STALE_AFTER_MS,
    db::{DatabaseConnector, DatabaseProbe, LiveDatabaseConnector},
    error::AppError,
    readiness::{CreateReadinessControllerOptions, ReadinessController, ReadinessPhase},
    router::{create_app, normalize_app},
    session::SessionService,
    state::AppState,
};

pub mod embedded_migrations {
    use refinery::embed_migrations;

    embed_migrations!("./migrations");
}

pub struct ServerOptions {
    pub host: String,
    pub port: u16,
    pub probe_interval_ms: u64,
    pub session_service: Arc<SessionService>,
    pub sql_connection_string: String,
    pub stale_after_ms: u64,
    pub web_root: Option<PathBuf>,
    pub database_connector: Option<Arc<dyn DatabaseConnector>>,
}

pub struct ServerInstance {
    pub app: axum::Router,
    pub readiness: ReadinessController,
    database: Option<Arc<dyn DatabaseProbe>>,
    database_connector: Arc<dyn DatabaseConnector>,
    host: String,
    port: u16,
    probe_interval_ms: u64,
    server_task: Option<JoinHandle<Result<(), std::io::Error>>>,
    shutdown_tx: Option<watch::Sender<bool>>,
    sql_connection_string: String,
    probe_task: Option<JoinHandle<()>>,
}

impl ServerInstance {
    pub fn new(options: ServerOptions) -> Self {
        let readiness = ReadinessController::new(CreateReadinessControllerOptions {
            stale_after_ms: Some(options.stale_after_ms),
            ..CreateReadinessControllerOptions::default()
        });
        let app = create_app(AppState {
            readiness: readiness.clone(),
            session_service: options.session_service,
            web_root: options.web_root,
        });

        Self {
            app,
            readiness,
            database: None,
            database_connector: options
                .database_connector
                .unwrap_or_else(|| Arc::new(LiveDatabaseConnector)),
            host: options.host,
            port: options.port,
            probe_interval_ms: options.probe_interval_ms,
            server_task: None,
            shutdown_tx: None,
            sql_connection_string: options.sql_connection_string,
            probe_task: None,
        }
    }

    pub async fn start(&mut self) -> Result<(), AppError> {
        self.readiness.mark_starting();

        let database = self
            .database_connector
            .connect(&self.sql_connection_string)
            .await
            .map_err(|error| {
                AppError::startup_with_source("Failed to initialize SQL.".to_string(), error)
            })?;

        verify_startup_probe(&self.readiness, database.as_ref()).await?;

        let listener_host: IpAddr = self.host.parse().map_err(|error| {
            AppError::config_with_source("Invalid listen host.".to_string(), error)
        })?;
        let listener = TcpListener::bind(SocketAddr::from((listener_host, self.port)))
            .await
            .map_err(|error| {
                AppError::startup_with_source(
                    "Failed to bind the ADE API server.".to_string(),
                    error,
                )
            })?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server = axum::serve(listener, Shared::new(normalize_app(self.app.clone())))
            .with_graceful_shutdown(wait_for_shutdown(shutdown_rx.clone()));
        let readiness = self.readiness.clone();
        let database_for_probe = Arc::clone(&database);
        let probe_interval_ms = self.probe_interval_ms;

        self.server_task = Some(tokio::spawn(async move { server.await }));
        self.probe_task = Some(tokio::spawn(async move {
            run_probe_loop(
                readiness,
                database_for_probe,
                shutdown_rx,
                probe_interval_ms,
            )
            .await;
        }));
        self.shutdown_tx = Some(shutdown_tx);
        self.database = Some(database);

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), AppError> {
        self.readiness.mark_stopping();

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        if let Some(probe_task) = self.probe_task.take() {
            probe_task.await.map_err(|error| {
                AppError::internal_with_source(
                    "Probe task failed to stop cleanly.".to_string(),
                    error,
                )
            })?;
        }

        if let Some(server_task) = self.server_task.take() {
            server_task
                .await
                .map_err(|error| {
                    AppError::internal_with_source(
                        "Server task failed to stop cleanly.".to_string(),
                        error,
                    )
                })?
                .map_err(|error| {
                    AppError::internal_with_source(
                        "Axum server exited with an IO error.".to_string(),
                        error,
                    )
                })?;
        }

        if let Some(database) = self.database.take() {
            database.close().await?;
        }

        Ok(())
    }
}

async fn verify_startup_probe(
    readiness: &ReadinessController,
    database: &dyn DatabaseProbe,
) -> Result<(), AppError> {
    match database.ping().await {
        Ok(()) => {
            readiness.record_database_success(unix_time_ms());
            readiness.mark_ready();
            Ok(())
        }
        Err(error) => {
            readiness.record_database_failure(unix_time_ms(), Some(&error.to_string()));
            readiness.mark_degraded(Some(&error.to_string()));
            Err(AppError::startup_with_source(
                "Failed to verify SQL connectivity during startup.".to_string(),
                error,
            ))
        }
    }
}

async fn wait_for_shutdown(mut shutdown_rx: watch::Receiver<bool>) {
    while shutdown_rx.changed().await.is_ok() {
        if *shutdown_rx.borrow() {
            return;
        }
    }
}

async fn run_probe_loop(
    readiness: ReadinessController,
    database: Arc<dyn DatabaseProbe>,
    mut shutdown_rx: watch::Receiver<bool>,
    probe_interval_ms: u64,
) {
    let mut ticker = interval(Duration::from_millis(probe_interval_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return;
                }
            }
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

pub fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

pub fn default_readiness_stale_after_ms() -> u64 {
    DEFAULT_READINESS_STALE_AFTER_MS
}

pub fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .try_init();
}

pub async fn run_server_until_shutdown(options: ServerOptions) -> Result<(), AppError> {
    let mut server = ServerInstance::new(options);
    server.start().await?;
    wait_for_termination_signal().await;
    server.stop().await
}

async fn wait_for_termination_signal() {
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        let mut signal = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        signal.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tempfile::tempdir;
    use tokio::time::sleep;

    use super::*;
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

    struct FakeConnector {
        database: Arc<dyn DatabaseProbe>,
    }

    #[async_trait]
    impl DatabaseConnector for FakeConnector {
        async fn connect(
            &self,
            _connection_string: &str,
        ) -> Result<Arc<dyn DatabaseProbe>, AppError> {
            Ok(Arc::clone(&self.database))
        }
    }

    fn fixture_session_service() -> Arc<SessionService> {
        let tempdir = tempdir().unwrap();
        let engine = tempdir.path().join("ade_engine-0.1.0-py3-none-any.whl");
        let config = tempdir.path().join("ade_config-0.1.0-py3-none-any.whl");
        std::fs::write(&engine, b"engine").unwrap();
        std::fs::write(&config, b"config").unwrap();

        let env = [
            (
                "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT".to_string(),
                "http://127.0.0.1:9".to_string(),
            ),
            (
                "ADE_SESSION_SECRET".to_string(),
                "test-session-secret".to_string(),
            ),
            (
                "ADE_ENGINE_WHEEL_PATH".to_string(),
                engine.display().to_string(),
            ),
            (
                "ADE_CONFIG_TARGETS".to_string(),
                serde_json::json!([
                    {
                        "workspaceId": "workspace-a",
                        "configVersionId": "config-v1",
                        "wheelPath": config.display().to_string(),
                    }
                ])
                .to_string(),
            ),
        ]
        .into_iter()
        .collect();

        SessionService::from_env(&env).unwrap()
    }

    #[tokio::test]
    async fn startup_fails_when_initial_probe_fails() {
        let database: Arc<dyn DatabaseProbe> =
            Arc::new(FakeDatabase::new(vec![Err("sql unavailable".to_string())]));
        let mut server = ServerInstance::new(ServerOptions {
            host: "127.0.0.1".to_string(),
            port: 0,
            probe_interval_ms: 10,
            session_service: fixture_session_service(),
            sql_connection_string: "unused".to_string(),
            stale_after_ms: 15_000,
            web_root: None,
            database_connector: Some(Arc::new(FakeConnector { database })),
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
        let mut server = ServerInstance::new(ServerOptions {
            host: "127.0.0.1".to_string(),
            port: 0,
            probe_interval_ms: 10,
            session_service: fixture_session_service(),
            sql_connection_string: "unused".to_string(),
            stale_after_ms: 15_000,
            web_root: None,
            database_connector: Some(Arc::new(FakeConnector { database })),
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
