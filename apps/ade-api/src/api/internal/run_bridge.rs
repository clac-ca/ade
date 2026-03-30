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

use crate::{error::AppError, runs::RunService};

pub fn router() -> Router<crate::api::AppState> {
    Router::new().route("/run-bridges/{bridgeId}", get(connect))
}

async fn connect(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<BridgePath>,
    Query(query): Query<BridgeQuery>,
) -> Result<Response, AppError> {
    let ws = ws.map_err(|error| AppError::request(error.to_string()))?;
    let bridge_tx = run_service.claim_bridge(&path.bridge_id, &query.token)?;

    Ok(ws
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _ = bridge_tx.send(socket);
        }))
}

#[derive(Deserialize)]
struct BridgePath {
    #[serde(rename = "bridgeId")]
    bridge_id: String,
}

#[derive(Deserialize)]
struct BridgeQuery {
    token: String,
}
