use std::process;

use ade_api::{
    config::{current_env, read_migration_config},
    db::run_migrations,
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

async fn run() -> Result<(), ade_api::error::AppError> {
    let env = current_env();
    let config = read_migration_config(&env)?;
    let applied = run_migrations(&config.sql_connection_string).await?;

    if applied.is_empty() {
        tracing::info!("No SQL migrations were pending.");
    } else {
        tracing::info!(count = applied.len(), "Applied SQL migrations.");
    }

    Ok(())
}
