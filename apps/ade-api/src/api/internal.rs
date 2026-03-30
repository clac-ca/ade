use axum::Router;

pub mod artifacts;
pub mod run_bridge;
pub mod terminal_bridge;

pub fn router() -> Router<crate::api::AppState> {
    terminal_bridge::router()
        .merge(run_bridge::router())
        .merge(artifacts::router())
}
