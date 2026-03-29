use std::{process, sync::Arc};

use ade_api::{
    config::{
        AppConfig, DEFAULT_DEV_HOST, DEFAULT_PORT, DEFAULT_PROBE_INTERVAL_MS,
        DEFAULT_READINESS_STALE_AFTER_MS, DEFAULT_RUNTIME_HOST, EnvBag, default_web_root,
        is_production,
    },
    error::AppError,
    init_tracing, run_server_until_shutdown,
    server::ServerOptions,
    session::SessionService,
};
use clap::Parser;

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

    if let Err(error) = run().await {
        tracing::error!(error = %error, "ADE API failed to start.");
        process::exit(1);
    }
}

async fn run() -> Result<(), AppError> {
    let env: EnvBag = std::env::vars().collect();
    let config = AppConfig::from_env(&env)?;
    let args = ServerArgs::parse();
    let production = is_production(&env);
    let session_service = Arc::new(SessionService::from_env(&env)?);
    let web_root = {
        let web_root = default_web_root();
        web_root.exists().then_some(web_root)
    };

    run_server_until_shutdown(ServerOptions {
        host: args.host.unwrap_or_else(|| {
            if production {
                DEFAULT_RUNTIME_HOST.to_string()
            } else {
                DEFAULT_DEV_HOST.to_string()
            }
        }),
        port: args.port.unwrap_or(DEFAULT_PORT),
        probe_interval_ms: args.probe_interval_ms.unwrap_or(DEFAULT_PROBE_INTERVAL_MS),
        session_service,
        sql_connection_string: config.sql_connection_string,
        stale_after_ms: args
            .stale_after_ms
            .unwrap_or(DEFAULT_READINESS_STALE_AFTER_MS),
        web_root,
        database: None,
    })
    .await
}
