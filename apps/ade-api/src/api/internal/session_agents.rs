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

use crate::{error::AppError, session_agent::SessionAgentService};

pub fn router() -> Router<crate::api::AppState> {
    Router::new().route("/session-agents/{channelId}", get(connect))
}

async fn connect(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State(session_agent_service): State<Arc<SessionAgentService>>,
    Path(channel_id): Path<String>,
    Query(query): Query<SessionAgentQuery>,
) -> Result<Response, AppError> {
    let ws = ws.map_err(|error| AppError::request(error.to_string()))?;
    let bridge_tx = session_agent_service.claim_rendezvous(&channel_id, &query.token)?;
    Ok(ws
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _ = bridge_tx.send(socket);
        }))
}

#[derive(Deserialize)]
struct SessionAgentQuery {
    token: String,
}
