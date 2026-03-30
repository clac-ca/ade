use std::{process, sync::Arc};

use ade_api::{
    config::{
        AppConfig, DEFAULT_DEV_HOST, DEFAULT_PORT, DEFAULT_PROBE_INTERVAL_MS,
        DEFAULT_READINESS_STALE_AFTER_MS, DEFAULT_RUNTIME_HOST, EnvBag, default_web_root,
        is_production,
    },
    db::Database,
    error::AppError,
    init_tracing,
    runs::{RunService, SqlRunStore},
    server::{ServerInstance, ServerOptions},
    session::SessionService,
    terminal::TerminalService,
};
use clap::Parser;
use tokio::signal;

#[derive(Debug, Parser)]
struct ServerArgs {
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long = "probe-interval-ms")]
    probe_interval_ms: Option<u64>,
    #[arg(long = "stale-after-ms")]
    stale_after_ms: Option<u64>,
}

#[tokio::main]
async fn main() {
    init_tracing();

    let result: Result<(), AppError> = async {
        let env: EnvBag = std::env::vars().collect();
        let config = AppConfig::from_env(&env)?;
        let args = ServerArgs::parse();
        let production = is_production(&env);
        let database = Arc::new(Database::connect(&config.sql_connection_string).await?);
        let session_service = Arc::new(SessionService::from_env(&env)?);
        let run_store = Arc::new(SqlRunStore::new(Arc::clone(&database)));
        let run_service = Arc::new(RunService::from_env(
            &env,
            Arc::clone(&session_service),
            run_store,
        )?);
        let terminal_service = Arc::new(TerminalService::from_env(
            &env,
            Arc::clone(&session_service),
        )?);
        let web_root = {
            let web_root = default_web_root();
            web_root.exists().then_some(web_root)
        };

        let mut server = ServerInstance::new(ServerOptions {
            host: args.host.unwrap_or_else(|| {
                if production {
                    DEFAULT_RUNTIME_HOST.to_string()
                } else {
                    DEFAULT_DEV_HOST.to_string()
                }
            }),
            port: args.port.unwrap_or(DEFAULT_PORT),
            probe_interval_ms: args.probe_interval_ms.unwrap_or(DEFAULT_PROBE_INTERVAL_MS),
            run_service,
            terminal_service,
            stale_after_ms: args
                .stale_after_ms
                .unwrap_or(DEFAULT_READINESS_STALE_AFTER_MS),
            web_root,
            database,
        });
        server.start().await?;

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
            () = ctrl_c => {}
            () = terminate => {}
        }

        server.stop().await
    }
    .await;

    if let Err(error) = result {
        tracing::error!(error = %error, "ADE API failed to start.");
        process::exit(1);
    }
}
