pub mod api;
pub mod artifacts;
mod azure_auth;
pub mod config;
pub mod db;
pub mod error;
pub mod readiness;
pub mod runs;
pub mod sandbox_environment;
pub mod scope;
pub mod server;
pub mod session_pool;
pub mod terminal;

pub use api::{AppState, create_app};

pub use server::{ServerInstance, ServerOptions, init_tracing, unix_time_ms};

pub mod embedded_migrations {
    use refinery::embed_migrations;

    embed_migrations!("./migrations");
}
