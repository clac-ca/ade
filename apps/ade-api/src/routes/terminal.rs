use std::sync::Arc;

use axum::{
    Router,
    extract::{
        Path, Query, State,
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    response::Response,
    routing::get,
};
use serde::Deserialize;

use crate::{error::AppError, session::Scope, terminal::TerminalService};

pub fn workspace_router() -> Router<crate::router::AppState> {
    Router::new().route("/terminal", get(connect_terminal))
}

pub fn internal_router() -> Router<crate::router::AppState> {
    Router::new().route("/terminals/{channelId}", get(connect_internal_bridge))
}

async fn connect_terminal(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State(terminal_service): State<Arc<TerminalService>>,
    Path(scope): Path<Scope>,
) -> Result<Response, AppError> {
    let ws = ws.map_err(map_websocket_rejection)?;
    Ok(ws
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            terminal_service.run_browser_terminal(scope, socket).await;
        }))
}

async fn connect_internal_bridge(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State(terminal_service): State<Arc<TerminalService>>,
    Path(path): Path<BridgePath>,
    Query(query): Query<BridgeQuery>,
) -> Result<Response, AppError> {
    let ws = ws.map_err(map_websocket_rejection)?;
    let bridge_tx = terminal_service.claim_bridge(&path.channel_id, &query.token)?;
    Ok(ws
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            terminal_service
                .attach_bridge_socket(socket, bridge_tx)
                .await;
        }))
}

#[derive(Deserialize)]
struct BridgePath {
    #[serde(rename = "channelId")]
    channel_id: String,
}

#[derive(Deserialize)]
struct BridgeQuery {
    token: String,
}

fn map_websocket_rejection(error: WebSocketUpgradeRejection) -> AppError {
    AppError::request(error.to_string())
}
