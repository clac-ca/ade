use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{extract::ws::WebSocket, http::StatusCode};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::oneshot;

use crate::{error::AppError, unix_time_ms};

static CHANNEL_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Default, Clone)]
pub(crate) struct PendingTerminalManager {
    inner: Arc<Mutex<HashMap<String, PendingBridgeEntry>>>,
}

impl PendingTerminalManager {
    pub(crate) fn create(&self, channel_id: String, token: String) -> PendingBrowserTerminal {
        let (bridge_tx, bridge_rx) = oneshot::channel();
        let entry = PendingBridgeEntry { bridge_tx };

        self.inner
            .lock()
            .expect("pending terminal bridge lock poisoned")
            .insert(channel_id.clone(), entry);

        PendingBrowserTerminal {
            channel_id,
            bridge_rx,
            token,
        }
    }

    pub(crate) fn claim(&self, channel_id: &str) -> Result<oneshot::Sender<WebSocket>, AppError> {
        let Some(entry) = self
            .inner
            .lock()
            .expect("pending terminal bridge lock poisoned")
            .remove(channel_id)
        else {
            return Err(AppError::not_found("Terminal bridge not found."));
        };

        Ok(entry.bridge_tx)
    }

    pub(crate) fn cancel(&self, channel_id: &str) {
        let _ = self
            .inner
            .lock()
            .expect("pending terminal bridge lock poisoned")
            .remove(channel_id);
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.inner
            .lock()
            .expect("pending terminal bridge lock poisoned")
            .len()
    }
}

pub(crate) struct PendingBridgeEntry {
    pub(crate) bridge_tx: oneshot::Sender<WebSocket>,
}

pub(crate) struct PendingBrowserTerminal {
    pub(crate) channel_id: String,
    pub(crate) bridge_rx: oneshot::Receiver<WebSocket>,
    pub(crate) token: String,
}

pub(crate) fn create_bridge_token(secret: &str, channel_id: &str, expires_at_ms: u64) -> String {
    let signature = bridge_signature(secret, channel_id, expires_at_ms);
    format!("{expires_at_ms}.{}", hex::encode(signature))
}

pub(crate) fn verify_bridge_token(
    secret: &str,
    channel_id: &str,
    token: &str,
    now_ms: u64,
) -> Result<(), AppError> {
    let Some((expires_at_ms, signature_hex)) = token.split_once('.') else {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Invalid terminal bridge token.",
        ));
    };
    let expires_at_ms = expires_at_ms.parse::<u64>().map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid terminal bridge token.")
    })?;
    if now_ms > expires_at_ms {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Terminal bridge token expired.",
        ));
    }

    let signature = hex::decode(signature_hex).map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid terminal bridge token.")
    })?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(channel_id.as_bytes());
    mac.update(b":");
    mac.update(expires_at_ms.to_string().as_bytes());
    mac.verify_slice(&signature).map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid terminal bridge token.")
    })?;

    Ok(())
}

fn bridge_signature(secret: &str, channel_id: &str, expires_at_ms: u64) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(channel_id.as_bytes());
    mac.update(b":");
    mac.update(expires_at_ms.to_string().as_bytes());
    mac.finalize().into_bytes().to_vec()
}

pub(crate) fn generate_channel_id(secret: &str) -> String {
    let counter = CHANNEL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now_ms = unix_time_ms();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(now_ms.to_string().as_bytes());
    mac.update(b":");
    mac.update(counter.to_string().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}
