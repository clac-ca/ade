use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum TerminalClientMessage {
    Close,
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum TerminalServerMessage {
    Ready,
    Output { data: String },
    Error { message: String },
    Exit { code: Option<i32> },
}

pub(crate) async fn send_terminal_event(
    socket: &mut WebSocket,
    event: TerminalServerMessage,
) -> Result<(), AppError> {
    let payload = serde_json::to_string(&event).map_err(|error| {
        AppError::internal_with_source("Failed to encode a browser terminal event.", error)
    })?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|error| {
            AppError::internal_with_source("Failed to write to the browser websocket.", error)
        })
}

pub(crate) fn parse_client_message(text: &str) -> Result<TerminalClientMessage, AppError> {
    serde_json::from_str::<TerminalClientMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid terminal message: {error}")))
}
