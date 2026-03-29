use std::process;

use ade_api::{
    config::{AppConfig, EnvBag},
    db::run_migrations,
    error::AppError,
    init_tracing,
};

#[tokio::main]
async fn main() {
    init_tracing();

    if let Err(error) = run().await {
        tracing::error!(error = %error, "ADE SQL migrations failed.");
        process::exit(1);
    }
}

async fn run() -> Result<(), AppError> {
    let env: EnvBag = std::env::vars().collect();
    let config = AppConfig::from_env(&env)?;
    let applied = run_migrations(&config.sql_connection_string).await?;

    if applied.is_empty() {
        tracing::info!("No SQL migrations were pending.");
    } else {
        tracing::info!(count = applied.len(), "Applied SQL migrations.");
    }

    Ok(())
}
