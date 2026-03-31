use axum::Router;

pub mod session_agents;

pub fn router() -> Router<crate::api::AppState> {
    session_agents::router()
}
