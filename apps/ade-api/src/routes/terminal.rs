use std::sync::Arc;

use axum::{
    Router,
    extract::{
        Path, State,
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    response::Response,
    routing::get,
};

use crate::{error::AppError, session::Scope, terminal::TerminalService};

pub fn workspace_router() -> Router<crate::router::AppState> {
    Router::new().route("/terminal", get(connect_terminal))
}

#[utoipa::path(
    get,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/terminal",
    tag = "terminal",
    params(Scope),
    responses(
        (status = 101, description = "WebSocket upgrade for the interactive terminal"),
        (status = 400, description = "Invalid websocket request", body = crate::error::ErrorResponse),
        (status = 404, description = "Scope not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
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

fn map_websocket_rejection(error: WebSocketUpgradeRejection) -> AppError {
    AppError::request(error.to_string())
}
