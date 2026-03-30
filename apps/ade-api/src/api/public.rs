use axum::Router;

pub mod runs;
pub mod system;
pub mod terminal;
pub mod uploads;

pub fn scoped_router() -> Router<crate::api::AppState> {
    uploads::router()
        .merge(runs::router())
        .merge(terminal::router())
}
