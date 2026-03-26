use std::process;

use ade_api::{
    ServerOptions,
    config::{
        AppConfig, DEFAULT_DEV_HOST, DEFAULT_PORT, DEFAULT_PROBE_INTERVAL_MS,
        DEFAULT_READINESS_STALE_AFTER_MS, DEFAULT_RUNTIME_HOST, ReadConfigOptions, current_env,
        default_runtime_paths, is_production, read_config,
    },
    init_tracing, run_server_until_shutdown,
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

async fn run() -> Result<(), ade_api::error::AppError> {
    let env = current_env();
    let config = read_config(
        &env,
        ReadConfigOptions {
            require_sql: true,
            ..ReadConfigOptions::default()
        },
    )?;
    let args = ServerArgs::parse();
    let runtime_paths = default_runtime_paths();

    let AppConfig {
        build_info,
        sql_connection_string,
    } = config;
    let sql_connection_string =
        sql_connection_string.expect("required SQL connection string missing");
    let web_root = runtime_paths
        .web_root
        .exists()
        .then_some(runtime_paths.web_root);

    run_server_until_shutdown(ServerOptions {
        build_info,
        host: args.host.unwrap_or_else(|| {
            if is_production(&env) {
                DEFAULT_RUNTIME_HOST.to_string()
            } else {
                DEFAULT_DEV_HOST.to_string()
            }
        }),
        port: args.port.unwrap_or(DEFAULT_PORT),
        probe_interval_ms: args.probe_interval_ms.unwrap_or(DEFAULT_PROBE_INTERVAL_MS),
        sql_connection_string,
        stale_after_ms: args
            .stale_after_ms
            .unwrap_or(DEFAULT_READINESS_STALE_AFTER_MS),
        web_root,
        database_connector: None,
    })
    .await
}
