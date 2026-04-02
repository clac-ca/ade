use std::sync::Arc;

use axum::{
    Router,
    extract::{
        Path, State,
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::header,
    response::Response,
    routing::get,
};

use crate::{error::AppError, sandbox_environment::SandboxEnvironmentManager};

pub fn router() -> Router<crate::api::AppState> {
    Router::new().route("/reverse-connect/{channelId}", get(connect))
}

async fn connect(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State(sandbox_environment_manager): State<Arc<SandboxEnvironmentManager>>,
    Path(channel_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let ws = ws.map_err(|error| AppError::request(error.to_string()))?;
    let token = bearer_token(&headers)?;
    let socket_tx = sandbox_environment_manager.claim_rendezvous(&channel_id, token)?;
    Ok(ws
        .protocols([::reverse_connect::protocol::WEBSOCKET_SUBPROTOCOL])
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _ = socket_tx.send(socket);
        }))
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Result<&str, AppError> {
    let header = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            AppError::status(
                axum::http::StatusCode::UNAUTHORIZED,
                "Missing bearer token.",
            )
        })?;
    header
        .strip_prefix("Bearer ")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::status(
                axum::http::StatusCode::UNAUTHORIZED,
                "Missing bearer token.",
            )
        })
}
