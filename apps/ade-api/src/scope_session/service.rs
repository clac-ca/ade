use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::extract::ws::{Message, WebSocket};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::Url;
use serde_json::Value;
use tokio::{
    fs,
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
    session_pool::{SessionExecution, SessionPoolClient},
    unix_time_ms,
};

use super::{
    bundle::{ScopeSessionId, SessionBundle, SessionBundleSource},
    rendezvous::{
        PendingConnectorManager, create_rendezvous_token, generate_channel_id,
        verify_rendezvous_token,
    },
};
use ::reverse_connect::protocol::{
    CHANNEL_CLOSE_METHOD, CHANNEL_DATA_METHOD, CHANNEL_EXIT_METHOD, CHANNEL_OPEN_METHOD,
    CHANNEL_RESIZE_METHOD, CHANNEL_SIGNAL_METHOD, CHANNEL_STDIN_METHOD, CONNECTOR_HELLO_METHOD,
    ChannelCloseParams, ChannelDataParams, ChannelExitParams, ChannelId, ChannelOpenParams,
    ChannelResizeParams, ChannelSignalParams, ChannelStdinParams, ChannelStream,
    ConnectorHelloParams, EmptyResult, RequestMessage, ResponseMessage, RpcMessage,
    SESSION_ERROR_METHOD, SessionErrorParams, SignalName,
};

const PUBLIC_API_URL_ENV_NAME: &str = "ADE_PUBLIC_API_URL";
const CONNECTOR_READY_TIMEOUT: Duration = Duration::from_secs(45);
const CONNECTOR_IDLE_SHUTDOWN_SECONDS: u64 = 15;
const CONNECTOR_RENDEZVOUS_TOKEN_TTL_MS: u64 = 60_000;
const CONNECTOR_EXECUTION_TIMEOUT_SECONDS: u64 = 3_600;

#[derive(Clone)]
pub struct ScopeSession {
    commands: mpsc::Sender<OutboundRpc>,
    events: broadcast::Sender<ScopeSessionEvent>,
    prepare_lock: Arc<tokio::sync::Mutex<()>>,
    prepared_revision: Arc<tokio::sync::Mutex<Option<String>>>,
    python_executable_path: String,
    run_script_path: String,
    session_root: String,
}

impl ScopeSession {
    pub fn is_closed(&self) -> bool {
        self.commands.is_closed()
    }

    pub async fn open_channel(&self, params: ChannelOpenParams) -> Result<(), AppError> {
        self.request(CHANNEL_OPEN_METHOD, params).await.map(|_| ())
    }

    pub async fn send_stdin(&self, channel_id: ChannelId, data: Vec<u8>) -> Result<(), AppError> {
        self.notify(
            CHANNEL_STDIN_METHOD,
            ChannelStdinParams {
                channel_id,
                data: STANDARD.encode(data),
            },
        )
        .await
    }

    pub async fn resize_pty(
        &self,
        channel_id: ChannelId,
        cols: u16,
        rows: u16,
    ) -> Result<(), AppError> {
        self.notify(
            CHANNEL_RESIZE_METHOD,
            ChannelResizeParams {
                channel_id,
                cols,
                rows,
            },
        )
        .await
    }

    pub async fn signal_channel(
        &self,
        channel_id: ChannelId,
        signal: SignalName,
    ) -> Result<(), AppError> {
        self.notify(
            CHANNEL_SIGNAL_METHOD,
            ChannelSignalParams { channel_id, signal },
        )
        .await
    }

    pub async fn close_channel(&self, channel_id: ChannelId) -> Result<(), AppError> {
        self.notify(CHANNEL_CLOSE_METHOD, ChannelCloseParams { channel_id })
            .await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ScopeSessionEvent> {
        self.events.subscribe()
    }

    pub fn python_executable_path(&self) -> &str {
        &self.python_executable_path
    }

    pub fn run_script_path(&self) -> &str {
        &self.run_script_path
    }

    pub fn session_root(&self) -> &str {
        &self.session_root
    }

    async fn notify(
        &self,
        method: &'static str,
        params: impl serde::Serialize,
    ) -> Result<(), AppError> {
        let params = serde_json::to_value(params).map_err(|error| {
            AppError::internal_with_source(
                "Failed to encode a reverse-connect notification.",
                error,
            )
        })?;
        self.commands
            .send(OutboundRpc::Notification { method, params })
            .await
            .map_err(|_| {
                AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Scope session connector is unavailable.",
                )
            })
    }

    async fn request(
        &self,
        method: &'static str,
        params: impl serde::Serialize,
    ) -> Result<Value, AppError> {
        let params = serde_json::to_value(params).map_err(|error| {
            AppError::internal_with_source("Failed to encode a reverse-connect request.", error)
        })?;
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(OutboundRpc::Request {
                method,
                params,
                response: response_tx,
            })
            .await
            .map_err(|_| {
                AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Scope session connector is unavailable.",
                )
            })?;
        match response_rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(AppError::status(
                axum::http::StatusCode::BAD_GATEWAY,
                message,
            )),
            Err(_) => Err(AppError::status(
                axum::http::StatusCode::BAD_GATEWAY,
                "Scope session connector request was cancelled.",
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ScopeSessionEvent {
    Data {
        channel_id: ChannelId,
        data: Vec<u8>,
        stream: ChannelStream,
    },
    Error {
        channel_id: Option<ChannelId>,
        message: String,
    },
    Exit {
        channel_id: ChannelId,
        code: Option<i32>,
    },
    Ready(ConnectorHelloParams),
}

#[derive(Clone)]
pub struct ScopeSessionService {
    app_url: Url,
    bundle_source: SessionBundleSource,
    manager: PendingConnectorManager,
    pool_client: SessionPoolClient,
    sessions: Arc<Mutex<HashMap<ScopeSessionId, SessionEntry>>>,
    session_secret: String,
}

enum SessionEntry {
    Ready(ScopeSession),
    Starting(Vec<oneshot::Sender<Result<ScopeSession, String>>>),
}

enum OutboundRpc {
    Notification {
        method: &'static str,
        params: Value,
    },
    Request {
        method: &'static str,
        params: Value,
        response: oneshot::Sender<Result<Value, String>>,
    },
}

impl ScopeSessionService {
    pub fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let app_url = read_optional_trimmed_string(env, PUBLIC_API_URL_ENV_NAME).ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {PUBLIC_API_URL_ENV_NAME}"
            ))
        })?;
        let app_url = Url::parse(&app_url).map_err(|error| {
            AppError::config_with_source(
                "ADE_PUBLIC_API_URL is not a valid URL.".to_string(),
                error,
            )
        })?;
        match app_url.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(AppError::config(
                    "ADE_PUBLIC_API_URL must use http or https.".to_string(),
                ));
            }
        }

        let bundle_source = SessionBundleSource::from_env(env)?;
        Self::new(app_url, bundle_source, SessionPoolClient::from_env(env)?)
    }

    pub(crate) fn new(
        app_url: Url,
        bundle_source: SessionBundleSource,
        pool_client: SessionPoolClient,
    ) -> Result<Self, AppError> {
        let session_secret = bundle_source.session_secret().to_string();

        Ok(Self {
            app_url,
            bundle_source,
            manager: PendingConnectorManager::default(),
            pool_client,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            session_secret,
        })
    }

    pub fn from_paths(
        app_url: &str,
        endpoint: &str,
        session_secret: &str,
        bundle_root: PathBuf,
        config_root: PathBuf,
    ) -> Result<Self, AppError> {
        let app_url = Url::parse(app_url).map_err(|error| {
            AppError::config_with_source(
                "ADE_PUBLIC_API_URL is not a valid URL.".to_string(),
                error,
            )
        })?;
        let bundle_env = [(
            "ADE_SCOPE_SESSION_SECRET".to_string(),
            session_secret.to_string(),
        )]
        .into_iter()
        .collect();
        let pool_env = [(
            "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT".to_string(),
            endpoint.to_string(),
        )]
        .into_iter()
        .collect();
        Self::new(
            app_url,
            SessionBundleSource::from_paths(bundle_root, config_root, &bundle_env)?,
            SessionPoolClient::from_env(&pool_env)?,
        )
    }

    pub(crate) fn claim_rendezvous(
        &self,
        channel_id: &str,
        token: &str,
    ) -> Result<oneshot::Sender<WebSocket>, AppError> {
        verify_rendezvous_token(&self.session_secret, channel_id, token, unix_time_ms())?;
        self.manager.claim(channel_id)
    }

    pub(crate) async fn ensure_ready_scope_session(
        &self,
        scope: &Scope,
    ) -> Result<ScopeSession, AppError> {
        let bundle = self.bundle_source.bundle_for_scope(scope)?;
        let handle = self.connect_scope_session(scope, &bundle).await?;
        self.prepare_scope_session(&handle, &bundle).await?;
        Ok(handle)
    }

    async fn connect_scope_session(
        &self,
        scope: &Scope,
        bundle: &SessionBundle,
    ) -> Result<ScopeSession, AppError> {
        let scope_session_id = self.bundle_source.scope_session_id(scope);

        loop {
            let mut should_launch = false;
            let mut wait_rx = None;
            {
                let mut sessions = self.sessions.lock().expect("scope session lock poisoned");
                match sessions.get_mut(&scope_session_id) {
                    Some(SessionEntry::Ready(handle)) if !handle.is_closed() => {
                        return Ok(handle.clone());
                    }
                    Some(SessionEntry::Ready(_)) => {
                        sessions.remove(&scope_session_id);
                        continue;
                    }
                    Some(SessionEntry::Starting(waiters)) => {
                        let (tx, rx) = oneshot::channel();
                        waiters.push(tx);
                        wait_rx = Some(rx);
                    }
                    None => {
                        sessions
                            .insert(scope_session_id.clone(), SessionEntry::Starting(Vec::new()));
                        should_launch = true;
                    }
                }
            }

            if let Some(wait_rx) = wait_rx {
                let result = wait_rx.await.map_err(|_| {
                    AppError::internal("Scope session startup wait channel closed unexpectedly.")
                })?;
                return result.map_err(AppError::unavailable);
            }

            if should_launch {
                let result = self
                    .launch_scope_session(scope, &scope_session_id, bundle)
                    .await;
                let mut waiters = Vec::new();
                {
                    let mut sessions = self.sessions.lock().expect("scope session lock poisoned");
                    match sessions.remove(&scope_session_id) {
                        Some(SessionEntry::Starting(pending)) => waiters = pending,
                        Some(SessionEntry::Ready(handle)) => return Ok(handle),
                        None => {}
                    }
                    if let Ok(handle) = result.as_ref() {
                        sessions.insert(
                            scope_session_id.clone(),
                            SessionEntry::Ready(handle.clone()),
                        );
                    }
                }

                match result {
                    Ok(handle) => {
                        for waiter in waiters {
                            let _ = waiter.send(Ok(handle.clone()));
                        }
                        return Ok(handle);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        for waiter in waiters {
                            let _ = waiter.send(Err(message.clone()));
                        }
                        return Err(error);
                    }
                }
            }
        }
    }

    async fn prepare_scope_session(
        &self,
        handle: &ScopeSession,
        bundle: &SessionBundle,
    ) -> Result<(), AppError> {
        let _guard = handle.prepare_lock.lock().await;
        {
            let prepared = handle.prepared_revision.lock().await;
            if prepared
                .as_ref()
                .is_some_and(|current| current == &bundle.prepare_revision)
            {
                return Ok(());
            }
        }

        let channel_id = ChannelId::new(format!("prepare-{}", Uuid::new_v4().simple()));
        let mut events = handle.subscribe();
        handle
            .open_channel(ChannelOpenParams {
                channel_id: channel_id.clone(),
                command: format!("sh {}", shell_single_quote(&bundle.prepare_script_path)),
                cwd: Some(bundle.session_root.clone()),
                env: Default::default(),
                kind: ::reverse_connect::protocol::ChannelKind::Exec,
                pty: None,
            })
            .await?;

        let timeout = tokio::time::sleep(CONNECTOR_READY_TIMEOUT);
        tokio::pin!(timeout);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(ScopeSessionEvent::Data { channel_id: event_channel_id, data, stream }) if event_channel_id == channel_id => {
                            match stream {
                                ChannelStream::Stdout => stdout.extend_from_slice(&data),
                                ChannelStream::Stderr => stderr.extend_from_slice(&data),
                                ChannelStream::Pty => {}
                            }
                        }
                        Ok(ScopeSessionEvent::Exit { channel_id: event_channel_id, code }) if event_channel_id == channel_id => {
                            if code == Some(0) {
                                let mut prepared = handle.prepared_revision.lock().await;
                                *prepared = Some(bundle.prepare_revision.clone());
                                return Ok(());
                            }

                            return Err(AppError::status(
                                axum::http::StatusCode::BAD_GATEWAY,
                                prepare_failure_message(code, &stdout, &stderr),
                            ));
                        }
                        Ok(ScopeSessionEvent::Error { channel_id: Some(event_channel_id), message }) if event_channel_id == channel_id => {
                            return Err(AppError::status(axum::http::StatusCode::BAD_GATEWAY, message));
                        }
                        Ok(ScopeSessionEvent::Error { channel_id: None, message }) => {
                            return Err(AppError::status(axum::http::StatusCode::BAD_GATEWAY, message));
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            return Err(AppError::status(
                                axum::http::StatusCode::BAD_GATEWAY,
                                "Scope session connector event stream overflowed during prepare.",
                            ));
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(AppError::status(
                                axum::http::StatusCode::BAD_GATEWAY,
                                "Scope session connector disconnected during prepare.",
                            ));
                        }
                    }
                }
                _ = &mut timeout => {
                    let _ = handle.close_channel(channel_id.clone()).await;
                    return Err(AppError::status(
                        axum::http::StatusCode::BAD_GATEWAY,
                        "Timed out waiting for the scope session to prepare.",
                    ));
                }
            }
        }
    }

    async fn launch_scope_session(
        &self,
        scope: &Scope,
        scope_session_id: &ScopeSessionId,
        bundle: &SessionBundle,
    ) -> Result<ScopeSession, AppError> {
        self.upload_bundle_files(scope, bundle).await?;

        let channel_id = generate_channel_id(&self.session_secret);
        let token = create_rendezvous_token(
            &self.session_secret,
            &channel_id,
            unix_time_ms() + CONNECTOR_RENDEZVOUS_TOKEN_TTL_MS,
        );
        let socket_rx = self.manager.create(channel_id.clone());
        let mut rendezvous_url = self.app_url.clone();
        let scheme = if rendezvous_url.scheme() == "http" {
            "ws"
        } else {
            "wss"
        };
        rendezvous_url
            .set_scheme(scheme)
            .expect("ADE_PUBLIC_API_URL scheme was validated at startup");
        rendezvous_url.set_path(&format!("/api/internal/reverse-connect/{channel_id}"));
        rendezvous_url.set_query(None);

        let command = render_launch_command(
            &bundle.connector_path,
            rendezvous_url.as_ref(),
            &token,
            CONNECTOR_IDLE_SHUTDOWN_SECONDS,
        );

        let pool_client = self.pool_client.clone();
        let identifier = scope_session_id.as_str().to_string();
        let mut execution_task = tokio::spawn(async move {
            pool_client
                .execute(
                    &identifier,
                    command,
                    Some(CONNECTOR_EXECUTION_TIMEOUT_SECONDS),
                )
                .await
                .map(|result| result.value)
        });

        let connector_socket = self
            .wait_for_connector_socket(&channel_id, socket_rx, &mut execution_task)
            .await?;

        let (events_tx, _) = broadcast::channel(256);
        let (command_tx, command_rx) = mpsc::channel(256);
        let handle = ScopeSession {
            commands: command_tx,
            events: events_tx.clone(),
            prepare_lock: Arc::new(tokio::sync::Mutex::new(())),
            prepared_revision: Arc::new(tokio::sync::Mutex::new(None)),
            python_executable_path: bundle.python_executable_path.clone(),
            run_script_path: bundle.run_script_path.clone(),
            session_root: bundle.session_root.clone(),
        };
        let mut ready_rx = events_tx.subscribe();
        tokio::spawn(run_scope_session_task(
            connector_socket,
            command_rx,
            events_tx,
            execution_task,
        ));
        wait_for_connector_ready(&mut ready_rx).await?;
        Ok(handle)
    }

    async fn wait_for_connector_socket(
        &self,
        channel_id: &str,
        socket_rx: oneshot::Receiver<WebSocket>,
        execution_task: &mut JoinHandle<Result<SessionExecution, AppError>>,
    ) -> Result<WebSocket, AppError> {
        let startup_timeout = tokio::time::sleep(CONNECTOR_READY_TIMEOUT);
        tokio::pin!(startup_timeout);
        tokio::pin!(socket_rx);

        let result = tokio::select! {
            result = &mut socket_rx => {
                result.map_err(|_| AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Scope session connector cancelled before the control channel connected.",
                ))
            }
            result = &mut *execution_task => {
                self.manager.cancel(channel_id);
                Err(startup_execution_error(result))
            }
            _ = &mut startup_timeout => {
                self.manager.cancel(channel_id);
                Err(AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Timed out waiting for the scope session connector to connect.",
                ))
            }
        };

        if result.is_err() && !execution_task.is_finished() {
            execution_task.abort();
            let _ = execution_task.await;
        }

        result
    }

    async fn upload_bundle_files(
        &self,
        scope: &Scope,
        bundle: &SessionBundle,
    ) -> Result<(), AppError> {
        let scope_session_id = self.bundle_source.scope_session_id(scope);
        for file in &bundle.files {
            let content = fs::read(&file.local_path).await.map_err(|error| {
                AppError::io_with_source(
                    format!(
                        "Failed to read the session bundle file '{}'.",
                        file.local_path.display()
                    ),
                    error,
                )
            })?;
            let (path, filename) = split_upload_target(&file.session_path);
            self.pool_client
                .upload_file(
                    scope_session_id.as_str(),
                    path,
                    filename,
                    Some(file.content_type),
                    content,
                )
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn upload_scope_file(
        &self,
        scope: &Scope,
        session_path: String,
        content_type: &str,
        content: Vec<u8>,
    ) -> Result<(), AppError> {
        let scope_session_id = self.bundle_source.scope_session_id(scope);
        let (path, filename) = split_upload_target(&session_path);
        self.pool_client
            .upload_file(
                scope_session_id.as_str(),
                path,
                filename,
                Some(content_type),
                content,
            )
            .await?;
        Ok(())
    }
}

fn render_launch_command(
    connector_path: &str,
    url: &str,
    bearer_token: &str,
    idle_timeout_seconds: u64,
) -> String {
    format!(
        "set -eu\nchmod 755 {connector_path}\nexec {connector_path} connect --url {url} --bearer-token {bearer_token} --idle-timeout-seconds {idle_timeout_seconds}"
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

fn split_upload_target(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once('/') {
        Some((directory, filename)) if !directory.is_empty() => (Some(directory), filename),
        _ => (None, path),
    }
}

fn prepare_failure_message(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    if !stdout.is_empty() {
        return stdout;
    }
    format!(
        "Scope session prepare command exited with code {}.",
        code.unwrap_or_default()
    )
}

fn startup_execution_error(
    result: Result<Result<SessionExecution, AppError>, tokio::task::JoinError>,
) -> AppError {
    match result {
        Ok(Ok(execution)) if matches!(execution.status.as_str(), "Success" | "Succeeded" | "0") => {
            let stderr = execution.stderr.trim();
            let stdout = execution.stdout.trim();
            let message = if !stderr.is_empty() {
                stderr.to_string()
            } else if !stdout.is_empty() {
                stdout.to_string()
            } else {
                "Scope session connector exited before the control channel connected.".to_string()
            };
            AppError::status(axum::http::StatusCode::BAD_GATEWAY, message)
        }
        Ok(Ok(execution)) => {
            let stderr = execution.stderr.trim();
            let stdout = execution.stdout.trim();
            let message = if !stderr.is_empty() {
                stderr.to_string()
            } else if !stdout.is_empty() {
                stdout.to_string()
            } else {
                format!(
                    "Scope session connector exited with status {}.",
                    execution.status
                )
            };
            AppError::status(axum::http::StatusCode::BAD_GATEWAY, message)
        }
        Ok(Err(error)) => error,
        Err(error) => AppError::status(
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Scope session execution task failed: {error}"),
        ),
    }
}

async fn wait_for_connector_ready(
    ready_rx: &mut broadcast::Receiver<ScopeSessionEvent>,
) -> Result<(), AppError> {
    let timeout = tokio::time::sleep(CONNECTOR_READY_TIMEOUT);
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            event = ready_rx.recv() => {
                match event {
                    Ok(ScopeSessionEvent::Ready(_)) => return Ok(()),
                    Ok(ScopeSessionEvent::Error { message, .. }) => {
                        return Err(AppError::status(axum::http::StatusCode::BAD_GATEWAY, message));
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        return Err(AppError::status(
                            axum::http::StatusCode::BAD_GATEWAY,
                            "Scope session connector event stream overflowed before startup completed.",
                        ));
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(AppError::status(
                            axum::http::StatusCode::BAD_GATEWAY,
                            "Scope session connector closed before becoming ready.",
                        ));
                    }
                }
            }
            _ = &mut timeout => {
                return Err(AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Timed out waiting for the scope session connector to become ready.",
                ));
            }
        }
    }
}

async fn run_scope_session_task(
    mut socket: WebSocket,
    mut commands: mpsc::Receiver<OutboundRpc>,
    events: broadcast::Sender<ScopeSessionEvent>,
    mut execution_task: JoinHandle<Result<SessionExecution, AppError>>,
) {
    let mut request_id = 10_u64;
    let mut pending = HashMap::<u64, oneshot::Sender<Result<Value, String>>>::new();

    loop {
        tokio::select! {
            maybe_command = commands.recv() => {
                let Some(command) = maybe_command else {
                    break;
                };
                let message = match command {
                    OutboundRpc::Notification { method, params } => {
                        RequestMessage::notification(method, params)
                    }
                    OutboundRpc::Request { method, params, response } => {
                        let id = request_id;
                        request_id += 1;
                        pending.insert(id, response);
                        RequestMessage::request(id, method, params)
                    }
                };
                let payload = match message.and_then(|message| serde_json::to_string(&RpcMessage::Request(message))) {
                    Ok(payload) => payload,
                    Err(error) => {
                        let _ = events.send(ScopeSessionEvent::Error {
                            channel_id: None,
                            message: format!("Failed to encode a reverse-connect message: {error}"),
                        });
                        break;
                    }
                };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    let _ = events.send(ScopeSessionEvent::Error {
                        channel_id: None,
                        message: "Scope session connector disconnected.".to_string(),
                    });
                    break;
                }
            }
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<RpcMessage>(text.as_str()) {
                            Ok(RpcMessage::Request(request)) => {
                                if handle_connector_request(&mut socket, &events, request).await.is_err() {
                                    break;
                                }
                            }
                            Ok(RpcMessage::Response(response)) => {
                                if let Some(waiter) = pending.remove(&response.id) {
                                    let _ = waiter.send(match response.error {
                                        Some(error) => Err(error.message),
                                        None => Ok(response.result.unwrap_or(Value::Null)),
                                    });
                                }
                            }
                            Err(error) => {
                                let _ = events.send(ScopeSessionEvent::Error {
                                    channel_id: None,
                                    message: format!("Invalid reverse-connect message: {error}"),
                                });
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = events.send(ScopeSessionEvent::Error {
                            channel_id: None,
                            message: "Scope session connector disconnected.".to_string(),
                        });
                        break;
                    }
                    Some(Ok(Message::Binary(_))) => {
                        let _ = events.send(ScopeSessionEvent::Error {
                            channel_id: None,
                            message: "Binary reverse-connect messages are not supported.".to_string(),
                        });
                        break;
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Err(error)) => {
                        let _ = events.send(ScopeSessionEvent::Error {
                            channel_id: None,
                            message: format!("Scope session websocket failed: {error}"),
                        });
                        break;
                    }
                }
            }
            result = &mut execution_task => {
                let message = match result {
                    Ok(Ok(execution)) if matches!(execution.status.as_str(), "Success" | "Succeeded" | "0") => None,
                    Ok(Ok(execution)) => {
                        Some(if execution.stderr.trim().is_empty() {
                            let stdout = execution.stdout.trim();
                            if stdout.is_empty() {
                                format!("Scope session connector exited with status {}.", execution.status)
                            } else {
                                stdout.to_string()
                            }
                        } else {
                            execution.stderr.trim().to_string()
                        })
                    }
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(error) => Some(format!("Scope session execution task failed: {error}")),
                };
                if let Some(message) = message {
                    let _ = events.send(ScopeSessionEvent::Error {
                        channel_id: None,
                        message,
                    });
                }
                break;
            }
        }
    }

    for (_, waiter) in pending {
        let _ = waiter.send(Err("Scope session connector disconnected.".to_string()));
    }
    let _ = socket.send(Message::Close(None)).await;
    if !execution_task.is_finished() {
        execution_task.abort();
        let _ = execution_task.await;
    }
}

async fn handle_connector_request(
    socket: &mut WebSocket,
    events: &broadcast::Sender<ScopeSessionEvent>,
    request: RequestMessage,
) -> Result<(), ()> {
    match request.method.as_str() {
        CONNECTOR_HELLO_METHOD => {
            let Some(id) = request.id else {
                let _ = events.send(ScopeSessionEvent::Error {
                    channel_id: None,
                    message: "connector.hello must be a request.".to_string(),
                });
                return Err(());
            };
            let response = match request.parse_params::<ConnectorHelloParams>() {
                Ok(params) => {
                    let _ = events.send(ScopeSessionEvent::Ready(params));
                    ResponseMessage::success(id, EmptyResult {}).expect("serializable hello ack")
                }
                Err(error) => ResponseMessage::invalid_params(id, error.to_string()),
            };
            let payload = serde_json::to_string(&RpcMessage::Response(response))
                .expect("serializable response");
            socket
                .send(Message::Text(payload.into()))
                .await
                .map_err(|_| ())?;
        }
        CHANNEL_DATA_METHOD => match request.parse_params::<ChannelDataParams>() {
            Ok(params) => {
                let data = match STANDARD.decode(&params.data) {
                    Ok(data) => data,
                    Err(error) => {
                        let _ = events.send(ScopeSessionEvent::Error {
                            channel_id: Some(params.channel_id),
                            message: format!("Invalid base64 channel data: {error}"),
                        });
                        return Ok(());
                    }
                };
                let _ = events.send(ScopeSessionEvent::Data {
                    channel_id: params.channel_id,
                    data,
                    stream: params.stream,
                });
            }
            Err(error) => {
                let _ = events.send(ScopeSessionEvent::Error {
                    channel_id: None,
                    message: format!("Invalid channel.data payload: {error}"),
                });
            }
        },
        CHANNEL_EXIT_METHOD => match request.parse_params::<ChannelExitParams>() {
            Ok(params) => {
                let _ = events.send(ScopeSessionEvent::Exit {
                    channel_id: params.channel_id,
                    code: params.code,
                });
            }
            Err(error) => {
                let _ = events.send(ScopeSessionEvent::Error {
                    channel_id: None,
                    message: format!("Invalid channel.exit payload: {error}"),
                });
            }
        },
        SESSION_ERROR_METHOD => match request.parse_params::<SessionErrorParams>() {
            Ok(params) => {
                let _ = events.send(ScopeSessionEvent::Error {
                    channel_id: params.channel_id,
                    message: params.message,
                });
            }
            Err(error) => {
                let _ = events.send(ScopeSessionEvent::Error {
                    channel_id: None,
                    message: format!("Invalid session.error payload: {error}"),
                });
            }
        },
        method => {
            if let Some(id) = request.id {
                let response = ResponseMessage::method_not_found(id, method);
                let payload = serde_json::to_string(&RpcMessage::Response(response))
                    .expect("serializable response");
                socket
                    .send(Message::Text(payload.into()))
                    .await
                    .map_err(|_| ())?;
            } else {
                let _ = events.send(ScopeSessionEvent::Error {
                    channel_id: None,
                    message: format!("Unsupported connector message '{method}'."),
                });
                return Err(());
            }
        }
    }

    Ok(())
}
