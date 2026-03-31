use axum::Router;

pub mod reverse_connect;

pub fn router() -> Router<crate::api::AppState> {
    reverse_connect::router()
}
