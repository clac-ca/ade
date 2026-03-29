pub mod api_docs;
pub mod artifacts;
pub mod config;
pub mod db;
pub mod error;
pub mod readiness;
pub mod router;
pub mod run_store;
pub mod runs;
pub mod server;
pub mod session;
pub mod terminal;

pub mod routes {
    pub mod runs;
    pub mod session;
    pub mod system;
    pub mod terminal;
}

pub use server::{
    ServerInstance, ServerOptions, init_tracing, run_server_until_shutdown, unix_time_ms,
};

pub mod embedded_migrations {
    use refinery::embed_migrations;

    embed_migrations!("./migrations");
}
