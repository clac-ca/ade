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
pub(crate) struct PendingConnectorManager {
    inner: Arc<Mutex<HashMap<String, oneshot::Sender<WebSocket>>>>,
}

impl PendingConnectorManager {
    pub(crate) fn create(&self, channel_id: String) -> oneshot::Receiver<WebSocket> {
        let (bridge_tx, bridge_rx) = oneshot::channel();
        self.inner
            .lock()
            .expect("reverse-connect rendezvous lock poisoned")
            .insert(channel_id, bridge_tx);
        bridge_rx
    }

    pub(crate) fn claim(&self, channel_id: &str) -> Result<oneshot::Sender<WebSocket>, AppError> {
        let Some(bridge_tx) = self
            .inner
            .lock()
            .expect("reverse-connect rendezvous lock poisoned")
            .remove(channel_id)
        else {
            return Err(AppError::not_found("Reverse-connect rendezvous not found."));
        };

        Ok(bridge_tx)
    }

    pub(crate) fn cancel(&self, channel_id: &str) {
        let _ = self
            .inner
            .lock()
            .expect("reverse-connect rendezvous lock poisoned")
            .remove(channel_id);
    }
}

pub(crate) fn create_rendezvous_token(
    secret: &str,
    channel_id: &str,
    expires_at_ms: u64,
) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(channel_id.as_bytes());
    mac.update(b":");
    mac.update(expires_at_ms.to_string().as_bytes());
    let signature = mac.finalize().into_bytes();
    format!("{expires_at_ms}.{}", hex::encode(signature))
}

pub(crate) fn verify_rendezvous_token(
    secret: &str,
    channel_id: &str,
    token: &str,
    now_ms: u64,
) -> Result<(), AppError> {
    let Some((expires_at_ms, signature_hex)) = token.split_once('.') else {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Invalid reverse-connect token.",
        ));
    };
    let expires_at_ms = expires_at_ms.parse::<u64>().map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid reverse-connect token.")
    })?;
    if now_ms > expires_at_ms {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Reverse-connect token expired.",
        ));
    }

    let signature = hex::decode(signature_hex).map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid reverse-connect token.")
    })?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(channel_id.as_bytes());
    mac.update(b":");
    mac.update(expires_at_ms.to_string().as_bytes());
    mac.verify_slice(&signature).map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid reverse-connect token.")
    })?;

    Ok(())
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
