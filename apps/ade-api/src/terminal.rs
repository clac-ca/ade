use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    extract::ws::{Message, WebSocket},
    http::StatusCode,
};
use hmac::{Hmac, Mac};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::{sync::oneshot, task::JoinHandle, time::Instant};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    session::{PythonExecution, Scope, SessionService},
    unix_time_ms,
};

const APP_URL_ENV_NAME: &str = "ADE_APP_URL";
const BRIDGE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const BRIDGE_TOKEN_TTL_MS: u64 = 60_000;
const DEFAULT_TERMINAL_COLS: u16 = 120;
const DEFAULT_TERMINAL_ROWS: u16 = 32;
const TERMINAL_EXECUTION_TIMEOUT_SECONDS: u64 = 220;

static CHANNEL_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct TerminalService {
    app_url: Url,
    manager: PendingTerminalManager,
    session_secret: String,
    session_service: Arc<SessionService>,
}

impl TerminalService {
    pub fn from_env(env: &EnvBag, session_service: Arc<SessionService>) -> Result<Self, AppError> {
        let app_url = read_optional_trimmed_string(env, APP_URL_ENV_NAME).ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {APP_URL_ENV_NAME}"
            ))
        })?;
        let parsed_app_url = Url::parse(&app_url).map_err(|error| {
            AppError::config_with_source("ADE_APP_URL is not a valid URL.".to_string(), error)
        })?;

        match parsed_app_url.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(AppError::config(
                    "ADE_APP_URL must use http or https.".to_string(),
                ));
            }
        }

        Ok(Self {
            app_url: parsed_app_url,
            manager: PendingTerminalManager::default(),
            session_secret: session_service.session_secret().to_string(),
            session_service,
        })
    }

    pub(crate) async fn run_browser_terminal(&self, scope: Scope, mut browser_socket: WebSocket) {
        let pending = self.create_pending_terminal();
        let bridge_url = match self.build_bridge_url(&pending.channel_id, &pending.token) {
            Ok(url) => url,
            Err(error) => {
                let _ = send_terminal_event(
                    &mut browser_socket,
                    TerminalServerMessage::error(error.to_string()),
                )
                .await;
                let _ = browser_socket.send(Message::Close(None)).await;
                return;
            }
        };

        let bootstrap_code = match render_bootstrap_code(&TerminalBootstrapConfig {
            bridge_url,
            cols: DEFAULT_TERMINAL_COLS,
            rows: DEFAULT_TERMINAL_ROWS,
        }) {
            Ok(code) => code,
            Err(error) => {
                let _ = send_terminal_event(
                    &mut browser_socket,
                    TerminalServerMessage::error(error.to_string()),
                )
                .await;
                let _ = browser_socket.send(Message::Close(None)).await;
                self.manager.cancel(&pending.channel_id);
                return;
            }
        };

        let mut execution_task =
            spawn_terminal_execution(Arc::clone(&self.session_service), scope, bootstrap_code);
        let startup_deadline = Instant::now() + BRIDGE_READY_TIMEOUT;

        let mut bridge_socket = match self
            .wait_for_bridge_socket(
                pending,
                &mut browser_socket,
                &mut execution_task,
                startup_deadline,
            )
            .await
        {
            Some(socket) => socket,
            None => return,
        };

        if !self
            .wait_for_ready_message(
                &mut browser_socket,
                &mut bridge_socket,
                &mut execution_task,
                startup_deadline,
            )
            .await
        {
            return;
        }

        self.relay_terminal_session(browser_socket, bridge_socket, execution_task)
            .await;
    }

    pub(crate) fn claim_bridge(
        &self,
        channel_id: &str,
        token: &str,
    ) -> Result<oneshot::Sender<WebSocket>, AppError> {
        verify_bridge_token(&self.session_secret, channel_id, token, unix_time_ms())?;
        self.manager.claim(channel_id)
    }

    pub(crate) async fn attach_bridge_socket(
        &self,
        socket: WebSocket,
        bridge_tx: oneshot::Sender<WebSocket>,
    ) {
        let _ = bridge_tx.send(socket);
    }

    pub(crate) fn build_bridge_url(
        &self,
        channel_id: &str,
        token: &str,
    ) -> Result<String, AppError> {
        let mut bridge_url = self.app_url.clone();
        let scheme = match bridge_url.scheme() {
            "http" => "ws",
            "https" => "wss",
            _ => {
                return Err(AppError::config(
                    "ADE_APP_URL must use http or https.".to_string(),
                ));
            }
        };
        bridge_url
            .set_scheme(scheme)
            .map_err(|()| AppError::internal("Failed to derive the terminal bridge URL."))?;
        bridge_url.set_path(&format!("/api/internal/terminals/{channel_id}"));
        bridge_url.set_query(None);
        bridge_url.query_pairs_mut().append_pair("token", token);
        Ok(bridge_url.to_string())
    }

    #[doc(hidden)]
    pub fn pending_count(&self) -> usize {
        self.manager.pending_count()
    }

    fn create_pending_terminal(&self) -> PendingBrowserTerminal {
        let channel_id = generate_channel_id(&self.session_secret);
        let token = create_bridge_token(
            &self.session_secret,
            &channel_id,
            unix_time_ms() + BRIDGE_TOKEN_TTL_MS,
        );
        self.manager.create(channel_id, token)
    }

    async fn wait_for_bridge_socket(
        &self,
        pending: PendingBrowserTerminal,
        browser_socket: &mut WebSocket,
        execution_task: &mut JoinHandle<Result<PythonExecution, AppError>>,
        startup_deadline: Instant,
    ) -> Option<WebSocket> {
        let startup_timeout = tokio::time::sleep_until(startup_deadline);
        tokio::pin!(startup_timeout);
        let bridge_rx = pending.bridge_rx;
        tokio::pin!(bridge_rx);

        loop {
            tokio::select! {
                bridge_result = &mut bridge_rx => {
                    return match bridge_result {
                        Ok(socket) => Some(socket),
                        Err(_) => {
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(
                                    "Terminal startup was cancelled before the bridge connected.".to_string(),
                                ),
                            ).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            None
                        }
                    };
                }
                browser_message = browser_socket.recv() => {
                    match BrowserStartupOutcome::from_message(browser_message) {
                        BrowserStartupOutcome::Ignore => {}
                        BrowserStartupOutcome::Disconnect => {
                            self.manager.cancel(&pending.channel_id);
                            execution_task.abort();
                            return None;
                        }
                        BrowserStartupOutcome::Error(message) => {
                            self.manager.cancel(&pending.channel_id);
                            execution_task.abort();
                            let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(message)).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return None;
                        }
                    }
                }
                result = &mut *execution_task => {
                    self.manager.cancel(&pending.channel_id);
                    let message = execution_failure_message(join_execution_result(result));
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(message)).await;
                    let _ = browser_socket.send(Message::Close(None)).await;
                    return None;
                }
                _ = &mut startup_timeout => {
                    self.manager.cancel(&pending.channel_id);
                    execution_task.abort();
                    let _ = send_terminal_event(
                        browser_socket,
                        TerminalServerMessage::error(
                            "Timed out waiting for the terminal bridge to connect.".to_string(),
                        ),
                    ).await;
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                    let _ = browser_socket.send(Message::Close(None)).await;
                    return None;
                }
            }
        }
    }

    async fn wait_for_ready_message(
        &self,
        browser_socket: &mut WebSocket,
        bridge_socket: &mut WebSocket,
        execution_task: &mut JoinHandle<Result<PythonExecution, AppError>>,
        startup_deadline: Instant,
    ) -> bool {
        let startup_timeout = tokio::time::sleep_until(startup_deadline);
        tokio::pin!(startup_timeout);

        loop {
            tokio::select! {
                bridge_message = bridge_socket.recv() => {
                    match bridge_message {
                        Some(Ok(Message::Text(text))) => {
                            let raw = text.to_string();
                            match parse_server_message(&raw) {
                                Ok(TerminalServerMessage::Ready) => {
                                    if browser_socket.send(Message::Text(raw.into())).await.is_err() {
                                        let _ = send_close_message(bridge_socket).await;
                                        return false;
                                    }
                                    return true;
                                }
                                Ok(_) => {
                                    let _ = send_terminal_event(
                                        browser_socket,
                                        TerminalServerMessage::error(
                                            "Terminal bridge must send a ready event before streaming output.".to_string(),
                                        ),
                                    ).await;
                                    let _ = send_close_message(bridge_socket).await;
                                    let _ = browser_socket.send(Message::Close(None)).await;
                                    return false;
                                }
                                Err(error) => {
                                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(error.to_string())).await;
                                    let _ = send_close_message(bridge_socket).await;
                                    let _ = browser_socket.send(Message::Close(None)).await;
                                    return false;
                                }
                            }
                        }
                        Some(Ok(Message::Binary(_))) => {
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(
                                    "Binary bridge messages are not supported.".to_string(),
                                ),
                            ).await;
                            let _ = send_close_message(bridge_socket).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return false;
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Close(_))) | None => {
                            let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return false;
                        }
                        Some(Err(error)) => {
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(format!("Terminal bridge failed: {error}")),
                            ).await;
                            let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return false;
                        }
                    }
                }
                browser_message = browser_socket.recv() => {
                    match BrowserStartupOutcome::from_message(browser_message) {
                        BrowserStartupOutcome::Ignore => {}
                        BrowserStartupOutcome::Disconnect => {
                            execution_task.abort();
                            let _ = send_close_message(bridge_socket).await;
                            return false;
                        }
                        BrowserStartupOutcome::Error(message) => {
                            execution_task.abort();
                            let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(message)).await;
                            let _ = send_close_message(bridge_socket).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return false;
                        }
                    }
                }
                result = &mut *execution_task => {
                    let message = execution_failure_message(join_execution_result(result));
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(message)).await;
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                    let _ = send_close_message(bridge_socket).await;
                    let _ = browser_socket.send(Message::Close(None)).await;
                    return false;
                }
                _ = &mut startup_timeout => {
                    execution_task.abort();
                    let _ = send_terminal_event(
                        browser_socket,
                        TerminalServerMessage::error(
                            "Timed out waiting for the terminal bridge to become ready.".to_string(),
                        ),
                    ).await;
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                    let _ = send_close_message(bridge_socket).await;
                    let _ = browser_socket.send(Message::Close(None)).await;
                    return false;
                }
            }
        }
    }

    async fn relay_terminal_session(
        &self,
        mut browser_socket: WebSocket,
        mut bridge_socket: WebSocket,
        mut execution_task: JoinHandle<Result<PythonExecution, AppError>>,
    ) {
        let session_timeout =
            tokio::time::sleep(Duration::from_secs(TERMINAL_EXECUTION_TIMEOUT_SECONDS));
        tokio::pin!(session_timeout);

        loop {
            tokio::select! {
                browser_message = browser_socket.recv() => {
                    match browser_message {
                        Some(Ok(message)) => {
                            match forward_browser_message(message, &mut bridge_socket).await {
                                Ok(TerminalRelayOutcome::Continue) => {}
                                Ok(TerminalRelayOutcome::Close) => break,
                                Err(error) => {
                                    let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::error(error.to_string())).await;
                                    let _ = send_close_message(&mut bridge_socket).await;
                                    break;
                                }
                            }
                        }
                        Some(Err(error)) => {
                            let _ = send_close_message(&mut bridge_socket).await;
                            let _ = send_terminal_event(
                                &mut browser_socket,
                                TerminalServerMessage::error(format!("Browser websocket failed: {error}")),
                            ).await;
                            break;
                        }
                        None => {
                            let _ = send_close_message(&mut bridge_socket).await;
                            break;
                        }
                    }
                }
                bridge_message = bridge_socket.recv() => {
                    match bridge_message {
                        Some(Ok(message)) => {
                            match forward_bridge_message(message, &mut browser_socket).await {
                                Ok(TerminalRelayOutcome::Continue) => {}
                                Ok(TerminalRelayOutcome::Close) => break,
                                Err(error) => {
                                    let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::error(error.to_string())).await;
                                    let _ = send_close_message(&mut bridge_socket).await;
                                    break;
                                }
                            }
                        }
                        Some(Err(error)) => {
                            let _ = send_terminal_event(
                                &mut browser_socket,
                                TerminalServerMessage::error(format!("Terminal bridge failed: {error}")),
                            ).await;
                            let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::exit(None)).await;
                            break;
                        }
                        None => {
                            let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::exit(None)).await;
                            break;
                        }
                    }
                }
                result = &mut execution_task => {
                    if let Some(message) = execution_error_message(&join_execution_result(result)) {
                        let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::error(message)).await;
                        let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::exit(None)).await;
                        let _ = send_close_message(&mut bridge_socket).await;
                        break;
                    }
                }
                _ = &mut session_timeout => {
                    let _ = send_terminal_event(
                        &mut browser_socket,
                        TerminalServerMessage::error(
                            "Terminal session expired after 220 seconds.".to_string(),
                        ),
                    ).await;
                    let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::exit(None)).await;
                    let _ = send_close_message(&mut bridge_socket).await;
                    break;
                }
            }
        }

        let _ = browser_socket.send(Message::Close(None)).await;
        execution_task.abort();
    }
}

#[derive(Default, Clone)]
struct PendingTerminalManager {
    inner: Arc<Mutex<HashMap<String, PendingBridgeEntry>>>,
}

impl PendingTerminalManager {
    fn create(&self, channel_id: String, token: String) -> PendingBrowserTerminal {
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

    fn claim(&self, channel_id: &str) -> Result<oneshot::Sender<WebSocket>, AppError> {
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

    fn cancel(&self, channel_id: &str) {
        let _ = self
            .inner
            .lock()
            .expect("pending terminal bridge lock poisoned")
            .remove(channel_id);
    }

    fn pending_count(&self) -> usize {
        self.inner
            .lock()
            .expect("pending terminal bridge lock poisoned")
            .len()
    }
}

struct PendingBridgeEntry {
    bridge_tx: oneshot::Sender<WebSocket>,
}

struct PendingBrowserTerminal {
    channel_id: String,
    bridge_rx: oneshot::Receiver<WebSocket>,
    token: String,
}

#[derive(Debug)]
enum BrowserStartupOutcome {
    Disconnect,
    Error(String),
    Ignore,
}

impl BrowserStartupOutcome {
    fn from_message(message: Option<Result<Message, axum::Error>>) -> Self {
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
enum TerminalRelayOutcome {
    Close,
    Continue,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum TerminalClientMessage {
    Close,
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum TerminalServerMessage {
    Ready,
    Output { data: String },
    Error { message: String },
    Exit { code: Option<i32> },
}

impl TerminalServerMessage {
    fn error(message: String) -> Self {
        Self::Error { message }
    }

    fn exit(code: Option<i32>) -> Self {
        Self::Exit { code }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalBootstrapConfig {
    bridge_url: String,
    cols: u16,
    rows: u16,
}

const BOOTSTRAP_TEMPLATE: &str = include_str!("terminal/bootstrap.py.tmpl");

fn render_bootstrap_code(config: &TerminalBootstrapConfig) -> Result<String, AppError> {
    if !BOOTSTRAP_TEMPLATE.contains("{{CONFIG_JSON}}") {
        return Err(AppError::internal(
            "Terminal bootstrap template is missing the CONFIG_JSON placeholder.",
        ));
    }

    let config_json = serde_json::to_string(config).map_err(|error| {
        AppError::internal_with_source(
            "Failed to encode the terminal bootstrap configuration.",
            error,
        )
    })?;

    let encoded = serde_json::to_string(&config_json).map_err(|error| {
        AppError::internal_with_source(
            "Failed to encode the terminal bootstrap JSON string.",
            error,
        )
    })?;

    Ok(BOOTSTRAP_TEMPLATE.replace("{{CONFIG_JSON}}", &encoded))
}

async fn send_terminal_event(
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

async fn send_close_message(socket: &mut WebSocket) -> Result<(), AppError> {
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

async fn forward_browser_message(
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

async fn forward_bridge_message(
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

fn parse_client_message(text: &str) -> Result<TerminalClientMessage, AppError> {
    serde_json::from_str::<TerminalClientMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid terminal message: {error}")))
}

fn parse_server_message(text: &str) -> Result<TerminalServerMessage, AppError> {
    serde_json::from_str::<TerminalServerMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid terminal bridge message: {error}")))
}

fn spawn_terminal_execution(
    session_service: Arc<SessionService>,
    scope: Scope,
    bootstrap_code: String,
) -> JoinHandle<Result<PythonExecution, AppError>> {
    tokio::spawn(async move {
        session_service
            .execute_inline_python(
                &scope,
                bootstrap_code,
                Some(TERMINAL_EXECUTION_TIMEOUT_SECONDS),
            )
            .await
    })
}

fn join_execution_result(
    result: Result<Result<PythonExecution, AppError>, tokio::task::JoinError>,
) -> Result<PythonExecution, AppError> {
    match result {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => {
            Err(AppError::internal("Terminal execution task was cancelled."))
        }
        Err(error) => Err(AppError::internal_with_source(
            "Terminal execution task failed to join.",
            error,
        )),
    }
}

fn create_bridge_token(secret: &str, channel_id: &str, expires_at_ms: u64) -> String {
    let signature = bridge_signature(secret, channel_id, expires_at_ms);
    format!("{expires_at_ms}.{}", hex::encode(signature))
}

fn verify_bridge_token(
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

fn generate_channel_id(secret: &str) -> String {
    let counter = CHANNEL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now_ms = unix_time_ms();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(now_ms.to_string().as_bytes());
    mac.update(b":");
    mac.update(counter.to_string().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn execution_error_message(result: &Result<PythonExecution, AppError>) -> Option<String> {
    match result {
        Ok(execution) if matches!(execution.status.as_str(), "Success" | "Succeeded" | "0") => None,
        Ok(execution) => Some(execution_failure_message(Ok(execution.clone()))),
        Err(error) => Some(error.to_string()),
    }
}

fn execution_failure_message(result: Result<PythonExecution, AppError>) -> String {
    match result {
        Ok(execution) => {
            if !execution.stderr.trim().is_empty() {
                return execution.stderr.trim().to_string();
            }
            if !execution.stdout.trim().is_empty() {
                return execution.stdout.trim().to_string();
            }
            format!(
                "Terminal execution failed with status {}.",
                execution.status
            )
        }
        Err(error) => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_app_url() {
        let tempdir = tempfile::tempdir().unwrap();
        let engine = tempdir.path().join("ade_engine-0.1.0-py3-none-any.whl");
        let config = tempdir.path().join("ade_config-0.1.0-py3-none-any.whl");
        std::fs::write(&engine, b"engine").unwrap();
        std::fs::write(&config, b"config").unwrap();
        let env = [
            (
                "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT".to_string(),
                "http://127.0.0.1:9".to_string(),
            ),
            (
                "ADE_SESSION_SECRET".to_string(),
                "test-session-secret".to_string(),
            ),
            (
                "ADE_ENGINE_WHEEL_PATH".to_string(),
                engine.display().to_string(),
            ),
            (
                "ADE_CONFIG_TARGETS".to_string(),
                serde_json::json!([
                    {
                        "workspaceId": "workspace-a",
                        "configVersionId": "config-v1",
                        "wheelPath": config.display().to_string(),
                    }
                ])
                .to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let session_service = Arc::new(SessionService::from_env(&env).unwrap());
        let error = TerminalService::from_env(&env, session_service)
            .err()
            .expect("missing ADE_APP_URL should fail");
        assert_eq!(
            error.to_string(),
            "Missing required environment variable: ADE_APP_URL"
        );
    }

    #[test]
    fn bridge_tokens_validate_and_expire() {
        let token = create_bridge_token("secret", "channel-a", 200);

        verify_bridge_token("secret", "channel-a", &token, 100).unwrap();
        assert!(verify_bridge_token("secret", "channel-b", &token, 100).is_err());
        assert!(verify_bridge_token("secret", "channel-a", &token, 201).is_err());
    }

    #[test]
    fn bootstrap_template_contains_bridge_and_pty_setup() {
        let code = render_bootstrap_code(&TerminalBootstrapConfig {
            bridge_url: "wss://example.com/api/internal/terminals/channel".to_string(),
            cols: 120,
            rows: 40,
        })
        .unwrap();

        assert!(code.contains("pty.openpty()"));
        assert!(code.contains("/mnt/data"));
        assert!(code.contains("websockets.connect"));
        assert!(code.contains("codecs.getincrementaldecoder"));
        assert!(code.contains("wss://example.com/api/internal/terminals/channel"));
    }

    #[test]
    fn pending_bridges_are_removed_on_cancel_and_claim() {
        let manager = PendingTerminalManager::default();
        let pending = manager.create("channel-a".to_string(), "token".to_string());

        assert_eq!(manager.pending_count(), 1);
        manager.cancel(&pending.channel_id);
        assert_eq!(manager.pending_count(), 0);

        let pending = manager.create("channel-b".to_string(), "token".to_string());
        let _attachment = manager.claim(&pending.channel_id).unwrap();
        assert_eq!(manager.pending_count(), 0);
    }
}
