use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    extract::ws::{Message, WebSocket},
    http::StatusCode,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::error::AppError;

use super::{RunPhase, RunValidationIssue};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum RunBridgeClientMessage {
    Error {
        #[serde(default)]
        phase: Option<RunPhase>,
        message: String,
        retriable: bool,
    },
    Log {
        level: String,
        message: String,
        phase: RunPhase,
    },
    Ready,
    Result {
        #[serde(rename = "outputPath")]
        output_path: String,
        #[serde(rename = "validationIssues")]
        validation_issues: Vec<RunValidationIssue>,
    },
    Status {
        phase: RunPhase,
        state: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum RunBridgeServerMessage {
    Cancel,
}

#[derive(Clone, Default)]
pub(crate) struct PendingRunBridgeManager {
    inner: Arc<Mutex<HashMap<String, PendingRunBridgeEntry>>>,
}

impl PendingRunBridgeManager {
    pub(crate) fn cancel(&self, bridge_id: &str) {
        self.inner
            .lock()
            .expect("pending run bridge lock poisoned")
            .remove(bridge_id);
    }

    pub(crate) fn claim(&self, bridge_id: &str) -> Result<oneshot::Sender<WebSocket>, AppError> {
        self.inner
            .lock()
            .expect("pending run bridge lock poisoned")
            .remove(bridge_id)
            .map(|entry| entry.bridge_tx)
            .ok_or_else(|| AppError::not_found("Run bridge not found."))
    }

    pub(crate) fn create(&self) -> PendingRunBridge {
        let bridge_id = Uuid::new_v4().simple().to_string();
        let (bridge_tx, bridge_rx) = oneshot::channel();

        self.inner
            .lock()
            .expect("pending run bridge lock poisoned")
            .insert(bridge_id.clone(), PendingRunBridgeEntry { bridge_tx });

        PendingRunBridge {
            bridge_id,
            bridge_rx,
        }
    }
}

pub(crate) struct PendingRunBridge {
    pub(crate) bridge_id: String,
    pub(crate) bridge_rx: oneshot::Receiver<WebSocket>,
}

struct PendingRunBridgeEntry {
    bridge_tx: oneshot::Sender<WebSocket>,
}

pub(crate) fn create_bridge_token(secret: &str, bridge_id: &str, expires_at_ms: u64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(bridge_id.as_bytes());
    mac.update(b":");
    mac.update(expires_at_ms.to_string().as_bytes());
    format!(
        "{expires_at_ms}.{}",
        hex::encode(mac.finalize().into_bytes())
    )
}

pub(crate) fn parse_bridge_message(text: &str) -> Result<RunBridgeClientMessage, AppError> {
    serde_json::from_str::<RunBridgeClientMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid run bridge message: {error}")))
}

pub(crate) async fn send_json(
    socket: &mut WebSocket,
    payload: &impl Serialize,
) -> Result<(), AppError> {
    let message = serde_json::to_string(payload).map_err(|error| {
        AppError::internal_with_source("Failed to encode a websocket payload.", error)
    })?;
    socket
        .send(Message::Text(message.into()))
        .await
        .map_err(|error| AppError::internal_with_source("Failed to write to a websocket.", error))
}

pub(crate) fn verify_bridge_token(
    secret: &str,
    bridge_id: &str,
    token: &str,
    now_ms: u64,
) -> Result<(), AppError> {
    let Some((expires_at_ms, signature_hex)) = token.split_once('.') else {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Invalid run bridge token.",
        ));
    };
    let expires_at_ms = expires_at_ms
        .parse::<u64>()
        .map_err(|_| AppError::status(StatusCode::UNAUTHORIZED, "Invalid run bridge token."))?;
    if now_ms > expires_at_ms {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Run bridge token expired.",
        ));
    }

    let signature = hex::decode(signature_hex)
        .map_err(|_| AppError::status(StatusCode::UNAUTHORIZED, "Invalid run bridge token."))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(bridge_id.as_bytes());
    mac.update(b":");
    mac.update(expires_at_ms.to_string().as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| AppError::status(StatusCode::UNAUTHORIZED, "Invalid run bridge token."))
}
