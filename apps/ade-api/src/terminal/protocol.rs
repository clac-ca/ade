use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug)]
pub(crate) enum BrowserStartupOutcome {
    Disconnect,
    Error(String),
    Ignore,
}

impl BrowserStartupOutcome {
    pub(crate) fn from_message(message: Option<Result<Message, axum::Error>>) -> Self {
        match message {
            Some(Ok(Message::Text(text))) => match parse_client_message(text.as_str()) {
                Ok(TerminalClientMessage::Close) => Self::Disconnect,
                Ok(_) => Self::Ignore,
                Err(error) => Self::Error(error.to_string()),
            },
            Some(Ok(Message::Binary(_))) => {
                Self::Error("Binary terminal messages are not supported.".to_string())
            }
            Some(Ok(Message::Close(_))) | None => Self::Disconnect,
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => Self::Ignore,
            Some(Err(error)) => Self::Error(format!("Browser websocket failed: {error}")),
        }
    }
}

#[derive(Debug)]
pub(crate) enum TerminalPhase<T> {
    AwaitExecution,
    Continue(T),
    Finished,
}

#[derive(Debug)]
pub(crate) enum TerminalRelayOutcome {
    Close,
    Continue,
}

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

impl TerminalServerMessage {
    pub(crate) fn error(message: String) -> Self {
        Self::Error { message }
    }

    pub(crate) fn exit(code: Option<i32>) -> Self {
        Self::Exit { code }
    }
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

pub(crate) async fn send_close_message(socket: &mut WebSocket) -> Result<(), AppError> {
    let payload = serde_json::to_string(&TerminalClientMessage::Close).map_err(|error| {
        AppError::internal_with_source("Failed to encode a close control message.", error)
    })?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|error| {
            AppError::internal_with_source(
                "Failed to write to the terminal bridge websocket.",
                error,
            )
        })
}

pub(crate) async fn forward_browser_message(
    message: Message,
    bridge_socket: &mut WebSocket,
) -> Result<TerminalRelayOutcome, AppError> {
    match message {
        Message::Text(text) => {
            let raw = text.to_string();
            match parse_client_message(&raw)? {
                TerminalClientMessage::Close => {
                    let _ = send_close_message(bridge_socket).await;
                    Ok(TerminalRelayOutcome::Close)
                }
                TerminalClientMessage::Input { .. } | TerminalClientMessage::Resize { .. } => {
                    bridge_socket
                        .send(Message::Text(raw.into()))
                        .await
                        .map_err(|error| {
                            AppError::internal_with_source(
                                "Failed to write to the terminal bridge websocket.",
                                error,
                            )
                        })?;
                    Ok(TerminalRelayOutcome::Continue)
                }
            }
        }
        Message::Binary(_) => Err(AppError::request(
            "Binary terminal messages are not supported.".to_string(),
        )),
        Message::Close(_) => {
            let _ = send_close_message(bridge_socket).await;
            Ok(TerminalRelayOutcome::Close)
        }
        Message::Ping(_) | Message::Pong(_) => Ok(TerminalRelayOutcome::Continue),
    }
}

pub(crate) async fn forward_bridge_message(
    message: Message,
    browser_socket: &mut WebSocket,
) -> Result<TerminalRelayOutcome, AppError> {
    match message {
        Message::Text(text) => {
            let raw = text.to_string();
            let event = parse_server_message(&raw)?;
            browser_socket
                .send(Message::Text(raw.into()))
                .await
                .map_err(|error| {
                    AppError::internal_with_source(
                        "Failed to write to the browser websocket.",
                        error,
                    )
                })?;
            if matches!(event, TerminalServerMessage::Exit { .. }) {
                return Ok(TerminalRelayOutcome::Close);
            }
            Ok(TerminalRelayOutcome::Continue)
        }
        Message::Binary(_) => Err(AppError::request(
            "Binary bridge messages are not supported.".to_string(),
        )),
        Message::Close(_) => {
            let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
            Ok(TerminalRelayOutcome::Close)
        }
        Message::Ping(_) | Message::Pong(_) => Ok(TerminalRelayOutcome::Continue),
    }
}

pub(crate) fn parse_client_message(text: &str) -> Result<TerminalClientMessage, AppError> {
    serde_json::from_str::<TerminalClientMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid terminal message: {error}")))
}

pub(crate) fn parse_server_message(text: &str) -> Result<TerminalServerMessage, AppError> {
    serde_json::from_str::<TerminalServerMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid terminal bridge message: {error}")))
}
