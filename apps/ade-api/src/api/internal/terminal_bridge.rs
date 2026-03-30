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

use crate::{error::AppError, terminal::TerminalService};

pub fn router() -> Router<crate::api::AppState> {
    Router::new().route("/terminals/{channelId}", get(connect))
}

async fn connect(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State(terminal_service): State<Arc<TerminalService>>,
    Path(channel_id): Path<String>,
    Query(query): Query<BridgeQuery>,
) -> Result<Response, AppError> {
    let ws = ws.map_err(|error| AppError::request(error.to_string()))?;
    let bridge_tx = terminal_service.claim_bridge(&channel_id, &query.token)?;
    Ok(ws
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _ = bridge_tx.send(socket);
        }))
}

#[derive(Deserialize)]
struct BridgeQuery {
    token: String,
}
