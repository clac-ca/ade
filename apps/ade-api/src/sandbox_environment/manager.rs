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
    session_pool::SessionExecution,
    unix_time_ms,
};

use super::{
    assets::{ConfigPackage, SandboxAssets, SandboxId},
    provider::SandboxProvider,
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
pub struct SandboxEnvironment {
    commands: mpsc::Sender<OutboundRpc>,
    events: broadcast::Sender<SandboxEnvironmentEvent>,
    ade_executable_path: String,
    python_executable_path: String,
    root_path: String,
    runtime_state: Arc<tokio::sync::Mutex<SandboxRuntimeState>>,
}

#[derive(Default)]
struct SandboxRuntimeState {
    installed_config_revision: Option<String>,
    prepared_environment_revision: Option<String>,
}

struct CapturedExec {
    code: Option<i32>,
    stderr: Vec<u8>,
    stdout: Vec<u8>,
}

impl SandboxEnvironment {
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

    pub fn subscribe(&self) -> broadcast::Receiver<SandboxEnvironmentEvent> {
        self.events.subscribe()
    }

    pub fn python_executable_path(&self) -> &str {
        &self.python_executable_path
    }

    pub fn ade_executable_path(&self) -> &str {
        &self.ade_executable_path
    }

    pub fn root_path(&self) -> &str {
        &self.root_path
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
                    "Sandbox environment connector is unavailable.",
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
                    "Sandbox environment connector is unavailable.",
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
                "Sandbox environment connector request was cancelled.",
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub enum SandboxEnvironmentEvent {
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
pub struct SandboxEnvironmentManager {
    app_url: Url,
    assets: SandboxAssets,
    manager: PendingConnectorManager,
    provider: SandboxProvider,
    sessions: Arc<Mutex<HashMap<SandboxId, SessionEntry>>>,
    sandbox_secret: String,
}

enum SessionEntry {
    Ready(SandboxEnvironment),
    Starting(Vec<oneshot::Sender<Result<SandboxEnvironment, String>>>),
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

impl SandboxEnvironmentManager {
    pub fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let app_url =
            read_optional_trimmed_string(env, PUBLIC_API_URL_ENV_NAME).ok_or_else(|| {
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

        let assets = SandboxAssets::from_env(env)?;
        Self::new(app_url, assets, SandboxProvider::from_env(env)?)
    }

    pub(crate) fn new(
        app_url: Url,
        assets: SandboxAssets,
        provider: SandboxProvider,
    ) -> Result<Self, AppError> {
        let sandbox_secret = assets.sandbox_secret().to_string();

        Ok(Self {
            app_url,
            assets,
            manager: PendingConnectorManager::default(),
            provider,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            sandbox_secret,
        })
    }

    pub fn from_paths(
        app_url: &str,
        endpoint: &str,
        sandbox_secret: &str,
        environment_archive: PathBuf,
        config_root: PathBuf,
    ) -> Result<Self, AppError> {
        let app_url = Url::parse(app_url).map_err(|error| {
            AppError::config_with_source(
                "ADE_PUBLIC_API_URL is not a valid URL.".to_string(),
                error,
            )
        })?;
        let sandbox_env = [(
            "ADE_SANDBOX_ENVIRONMENT_SECRET".to_string(),
            sandbox_secret.to_string(),
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
            SandboxAssets::from_paths(environment_archive, config_root, &sandbox_env)?,
            SandboxProvider::new(crate::session_pool::SessionPoolClient::from_env(&pool_env)?),
        )
    }

    pub(crate) fn claim_rendezvous(
        &self,
        channel_id: &str,
        token: &str,
    ) -> Result<oneshot::Sender<WebSocket>, AppError> {
        verify_rendezvous_token(&self.sandbox_secret, channel_id, token, unix_time_ms())?;
        self.manager.claim(channel_id)
    }

    pub(crate) async fn ensure_ready_environment(
        &self,
        scope: &Scope,
    ) -> Result<SandboxEnvironment, AppError> {
        let handle = self.allocate(scope).await?;
        self.prepare(scope, &handle).await?;
        self.install(scope, &handle).await?;
        Ok(handle)
    }

    pub(crate) async fn allocate(&self, scope: &Scope) -> Result<SandboxEnvironment, AppError> {
        self.connect_sandbox_environment(scope).await
    }

    pub(crate) async fn prepare(
        &self,
        _scope: &Scope,
        handle: &SandboxEnvironment,
    ) -> Result<(), AppError> {
        self.prepare_sandbox_environment(handle).await
    }

    pub(crate) async fn install(
        &self,
        scope: &Scope,
        handle: &SandboxEnvironment,
    ) -> Result<(), AppError> {
        let config = self.assets.config_for_scope(scope)?;
        self.install_config(scope, handle, &config).await
    }

    async fn connect_sandbox_environment(
        &self,
        scope: &Scope,
    ) -> Result<SandboxEnvironment, AppError> {
        let sandbox_id = self.assets.sandbox_id(scope);

        loop {
            let mut should_launch = false;
            let mut wait_rx = None;
            {
                let mut sessions = self.sessions.lock().expect("sandbox lock poisoned");
                match sessions.get_mut(&sandbox_id) {
                    Some(SessionEntry::Ready(handle)) if !handle.is_closed() => {
                        return Ok(handle.clone());
                    }
                    Some(SessionEntry::Ready(_)) => {
                        sessions.remove(&sandbox_id);
                        continue;
                    }
                    Some(SessionEntry::Starting(waiters)) => {
                        let (tx, rx) = oneshot::channel();
                        waiters.push(tx);
                        wait_rx = Some(rx);
                    }
                    None => {
                        sessions.insert(sandbox_id.clone(), SessionEntry::Starting(Vec::new()));
                        should_launch = true;
                    }
                }
            }

            if let Some(wait_rx) = wait_rx {
                let result = wait_rx.await.map_err(|_| {
                    AppError::internal(
                        "Sandbox environment startup wait channel closed unexpectedly.",
                    )
                })?;
                return result.map_err(AppError::unavailable);
            }

            if should_launch {
                let result = self.launch_sandbox_environment(scope, &sandbox_id).await;
                let mut waiters = Vec::new();
                {
                    let mut sessions = self.sessions.lock().expect("sandbox lock poisoned");
                    match sessions.remove(&sandbox_id) {
                        Some(SessionEntry::Starting(pending)) => waiters = pending,
                        Some(SessionEntry::Ready(handle)) => return Ok(handle),
                        None => {}
                    }
                    if let Ok(handle) = result.as_ref() {
                        sessions.insert(sandbox_id.clone(), SessionEntry::Ready(handle.clone()));
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

    async fn prepare_sandbox_environment(
        &self,
        handle: &SandboxEnvironment,
    ) -> Result<(), AppError> {
        let mut runtime_state = handle.runtime_state.lock().await;
        if runtime_state
            .prepared_environment_revision
            .as_ref()
            .is_some_and(|current| current == self.assets.environment_revision())
        {
            return Ok(());
        }

        let output = self
            .run_exec_command(
                handle,
                "setup",
                format!("sh {}", shell_single_quote(self.assets.setup_script_path())),
                "Timed out waiting for the sandbox environment to finish setup.",
                "Sandbox environment event stream overflowed during setup.",
                "Sandbox environment disconnected during setup.",
            )
            .await?;

        if output.code != Some(0) {
            return Err(AppError::status(
                axum::http::StatusCode::BAD_GATEWAY,
                setup_failure_message(output.code, &output.stdout, &output.stderr),
            ));
        }

        runtime_state.prepared_environment_revision =
            Some(self.assets.environment_revision().to_string());
        runtime_state.installed_config_revision = None;
        Ok(())
    }

    async fn install_config(
        &self,
        scope: &Scope,
        handle: &SandboxEnvironment,
        config: &ConfigPackage,
    ) -> Result<(), AppError> {
        let mut runtime_state = handle.runtime_state.lock().await;
        if runtime_state
            .installed_config_revision
            .as_ref()
            .is_some_and(|current| current == &config.install_revision)
        {
            return Ok(());
        }

        self.upload_config_package(scope, config).await?;

        let output = self
            .run_exec_command(
                handle,
                "config",
                format!(
                    "{} -m pip install --upgrade --no-index --find-links {} {}",
                    shell_single_quote(handle.python_executable_path()),
                    shell_single_quote(self.assets.base_wheelhouse_path()),
                    shell_single_quote(&config.mounted_path),
                ),
                "Timed out waiting for config installation to finish.",
                "Sandbox environment event stream overflowed during config installation.",
                "Sandbox environment disconnected during config installation.",
            )
            .await?;

        if output.code != Some(0) {
            return Err(AppError::status(
                axum::http::StatusCode::BAD_GATEWAY,
                config_install_failure_message(output.code, &output.stdout, &output.stderr),
            ));
        }

        runtime_state.installed_config_revision = Some(config.install_revision.clone());
        Ok(())
    }

    async fn run_exec_command(
        &self,
        handle: &SandboxEnvironment,
        channel_prefix: &str,
        command: String,
        timeout_message: &'static str,
        overflow_message: &'static str,
        disconnected_message: &'static str,
    ) -> Result<CapturedExec, AppError> {
        let channel_id = ChannelId::new(format!("{channel_prefix}-{}", Uuid::new_v4().simple()));
        let mut events = handle.subscribe();
        handle
            .open_channel(ChannelOpenParams {
                channel_id: channel_id.clone(),
                command,
                cwd: Some(handle.root_path().to_string()),
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
                        Ok(SandboxEnvironmentEvent::Data { channel_id: event_channel_id, data, stream }) if event_channel_id == channel_id => {
                            match stream {
                                ChannelStream::Stdout => stdout.extend_from_slice(&data),
                                ChannelStream::Stderr => stderr.extend_from_slice(&data),
                                ChannelStream::Pty => {}
                            }
                        }
                        Ok(SandboxEnvironmentEvent::Exit { channel_id: event_channel_id, code }) if event_channel_id == channel_id => {
                            return Ok(CapturedExec { code, stdout, stderr });
                        }
                        Ok(SandboxEnvironmentEvent::Error { channel_id: Some(event_channel_id), message }) if event_channel_id == channel_id => {
                            return Err(AppError::status(axum::http::StatusCode::BAD_GATEWAY, message));
                        }
                        Ok(SandboxEnvironmentEvent::Error { channel_id: None, message }) => {
                            return Err(AppError::status(axum::http::StatusCode::BAD_GATEWAY, message));
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            return Err(AppError::status(
                                axum::http::StatusCode::BAD_GATEWAY,
                                overflow_message,
                            ));
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(AppError::status(
                                axum::http::StatusCode::BAD_GATEWAY,
                                disconnected_message,
                            ));
                        }
                    }
                }
                _ = &mut timeout => {
                    let _ = handle.close_channel(channel_id.clone()).await;
                    return Err(AppError::status(
                        axum::http::StatusCode::BAD_GATEWAY,
                        timeout_message,
                    ));
                }
            }
        }
    }

    async fn launch_sandbox_environment(
        &self,
        scope: &Scope,
        sandbox_id: &SandboxId,
    ) -> Result<SandboxEnvironment, AppError> {
        self.upload_environment_archive(scope).await?;

        let channel_id = generate_channel_id(&self.sandbox_secret);
        let token = create_rendezvous_token(
            &self.sandbox_secret,
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
            &self.assets.archive_remote_path(),
            self.assets.connector_path(),
            rendezvous_url.as_ref(),
            &token,
            CONNECTOR_IDLE_SHUTDOWN_SECONDS,
        );

        let provider = self.provider.clone();
        let identifier = sandbox_id.as_str().to_string();
        let mut execution_task = tokio::spawn(async move {
            provider
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
        let handle = SandboxEnvironment {
            commands: command_tx,
            events: events_tx.clone(),
            ade_executable_path: self.assets.ade_executable_path().to_string(),
            python_executable_path: self.assets.python_executable_path().to_string(),
            root_path: self.assets.root_path().to_string(),
            runtime_state: Arc::new(tokio::sync::Mutex::new(SandboxRuntimeState::default())),
        };
        let mut ready_rx = events_tx.subscribe();
        tokio::spawn(run_sandbox_environment_task(
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
                    "Sandbox environment connector cancelled before the control channel connected.",
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
                    "Timed out waiting for the sandbox environment connector to connect.",
                ))
            }
        };

        if result.is_err() && !execution_task.is_finished() {
            execution_task.abort();
            let _ = execution_task.await;
        }

        result
    }

    async fn upload_environment_archive(&self, scope: &Scope) -> Result<(), AppError> {
        let sandbox_id = self.assets.sandbox_id(scope);
        let content = fs::read(self.assets.archive_local_path())
            .await
            .map_err(|error| {
                AppError::io_with_source(
                    format!(
                        "Failed to read the sandbox-environment archive '{}'.",
                        self.assets.archive_local_path().display()
                    ),
                    error,
                )
            })?;
        self.provider
            .upload_file(
                sandbox_id.as_str(),
                None,
                self.assets.archive_filename(),
                Some(self.assets.archive_content_type()),
                content,
            )
            .await?;
        Ok(())
    }

    async fn upload_config_package(
        &self,
        scope: &Scope,
        config: &ConfigPackage,
    ) -> Result<(), AppError> {
        let content = fs::read(&config.local_path).await.map_err(|error| {
            AppError::io_with_source(
                format!(
                    "Failed to read the config package '{}'.",
                    config.local_path.display()
                ),
                error,
            )
        })?;
        self.provider
            .upload_file(
                self.assets.sandbox_id(scope).as_str(),
                Some(&config.mounted_directory_path),
                &config.filename,
                Some("application/octet-stream"),
                content,
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn upload_sandbox_file(
        &self,
        scope: &Scope,
        sandbox_path: String,
        content_type: &str,
        content: Vec<u8>,
    ) -> Result<(), AppError> {
        let sandbox_id = self.assets.sandbox_id(scope);
        let (path, filename) = split_upload_target(&sandbox_path);
        self.provider
            .upload_file(
                sandbox_id.as_str(),
                path,
                filename,
                Some(content_type),
                content,
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn download_sandbox_file(
        &self,
        scope: &Scope,
        sandbox_directory: String,
        filename: String,
    ) -> Result<Vec<u8>, AppError> {
        let sandbox_id = self.assets.sandbox_id(scope);
        let directory = if sandbox_directory.is_empty() {
            None
        } else {
            Some(sandbox_directory.as_str())
        };
        self.provider
            .download_file(sandbox_id.as_str(), directory, &filename)
            .await
            .map(|result| result.value)
    }
}

fn render_launch_command(
    archive_path: &str,
    connector_path: &str,
    url: &str,
    bearer_token: &str,
    idle_timeout_seconds: u64,
) -> String {
    let archive_path = shell_single_quote(archive_path);
    let connector_path = shell_single_quote(connector_path);
    let url = shell_single_quote(url);
    let bearer_token = shell_single_quote(bearer_token);
    format!(
        "set -eu\ntar --keep-directory-symlink -xzf {archive_path} -C /\nchmod 755 {connector_path}\nexec {connector_path} connect --url {url} --bearer-token {bearer_token} --idle-timeout-seconds {idle_timeout_seconds}"
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

fn setup_failure_message(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    if !stdout.is_empty() {
        return stdout;
    }
    format!(
        "Sandbox environment setup command exited with code {}.",
        code.unwrap_or_default()
    )
}

fn config_install_failure_message(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    if !stdout.is_empty() {
        return stdout;
    }
    format!(
        "Sandbox config installation exited with code {}.",
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
                "Sandbox environment connector exited before the control channel connected."
                    .to_string()
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
                    "Sandbox environment connector exited with status {}.",
                    execution.status
                )
            };
            AppError::status(axum::http::StatusCode::BAD_GATEWAY, message)
        }
        Ok(Err(error)) => error,
        Err(error) => AppError::status(
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Sandbox environment execution task failed: {error}"),
        ),
    }
}

async fn wait_for_connector_ready(
    ready_rx: &mut broadcast::Receiver<SandboxEnvironmentEvent>,
) -> Result<(), AppError> {
    let timeout = tokio::time::sleep(CONNECTOR_READY_TIMEOUT);
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            event = ready_rx.recv() => {
                match event {
                    Ok(SandboxEnvironmentEvent::Ready(_)) => return Ok(()),
                    Ok(SandboxEnvironmentEvent::Error { message, .. }) => {
                        return Err(AppError::status(axum::http::StatusCode::BAD_GATEWAY, message));
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        return Err(AppError::status(
                            axum::http::StatusCode::BAD_GATEWAY,
                            "Sandbox environment event stream overflowed before startup completed.",
                        ));
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(AppError::status(
                            axum::http::StatusCode::BAD_GATEWAY,
                            "Sandbox environment connector closed before becoming ready.",
                        ));
                    }
                }
            }
            _ = &mut timeout => {
                return Err(AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Timed out waiting for the sandbox environment connector to become ready.",
                ));
            }
        }
    }
}

async fn run_sandbox_environment_task(
    mut socket: WebSocket,
    mut commands: mpsc::Receiver<OutboundRpc>,
    events: broadcast::Sender<SandboxEnvironmentEvent>,
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
                        let _ = events.send(SandboxEnvironmentEvent::Error {
                            channel_id: None,
                            message: format!("Failed to encode a reverse-connect message: {error}"),
                        });
                        break;
                    }
                };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    let _ = events.send(SandboxEnvironmentEvent::Error {
                        channel_id: None,
                        message: "Sandbox environment connector disconnected.".to_string(),
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
                                let _ = events.send(SandboxEnvironmentEvent::Error {
                                    channel_id: None,
                                    message: format!("Invalid reverse-connect message: {error}"),
                                });
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = events.send(SandboxEnvironmentEvent::Error {
                            channel_id: None,
                            message: "Sandbox environment connector disconnected.".to_string(),
                        });
                        break;
                    }
                    Some(Ok(Message::Binary(_))) => {
                        let _ = events.send(SandboxEnvironmentEvent::Error {
                            channel_id: None,
                            message: "Binary reverse-connect messages are not supported.".to_string(),
                        });
                        break;
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Err(error)) => {
                        let _ = events.send(SandboxEnvironmentEvent::Error {
                            channel_id: None,
                            message: format!(
                                "Sandbox environment websocket failed: {error}"
                            ),
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
                                format!(
                                    "Sandbox environment connector exited with status {}.",
                                    execution.status
                                )
                            } else {
                                stdout.to_string()
                            }
                        } else {
                            execution.stderr.trim().to_string()
                        })
                    }
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(error) => Some(format!(
                        "Sandbox environment execution task failed: {error}"
                    )),
                };
                if let Some(message) = message {
                    let _ = events.send(SandboxEnvironmentEvent::Error {
                        channel_id: None,
                        message,
                    });
                }
                break;
            }
        }
    }

    for (_, waiter) in pending {
        let _ = waiter.send(Err(
            "Sandbox environment connector disconnected.".to_string()
        ));
    }
    let _ = socket.send(Message::Close(None)).await;
    if !execution_task.is_finished() {
        execution_task.abort();
        let _ = execution_task.await;
    }
}

async fn handle_connector_request(
    socket: &mut WebSocket,
    events: &broadcast::Sender<SandboxEnvironmentEvent>,
    request: RequestMessage,
) -> Result<(), ()> {
    match request.method.as_str() {
        CONNECTOR_HELLO_METHOD => {
            let Some(id) = request.id else {
                let _ = events.send(SandboxEnvironmentEvent::Error {
                    channel_id: None,
                    message: "connector.hello must be a request.".to_string(),
                });
                return Err(());
            };
            let response = match request.parse_params::<ConnectorHelloParams>() {
                Ok(params) => {
                    let _ = events.send(SandboxEnvironmentEvent::Ready(params));
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
                        let _ = events.send(SandboxEnvironmentEvent::Error {
                            channel_id: Some(params.channel_id),
                            message: format!("Invalid base64 channel data: {error}"),
                        });
                        return Ok(());
                    }
                };
                let _ = events.send(SandboxEnvironmentEvent::Data {
                    channel_id: params.channel_id,
                    data,
                    stream: params.stream,
                });
            }
            Err(error) => {
                let _ = events.send(SandboxEnvironmentEvent::Error {
                    channel_id: None,
                    message: format!("Invalid channel.data payload: {error}"),
                });
            }
        },
        CHANNEL_EXIT_METHOD => match request.parse_params::<ChannelExitParams>() {
            Ok(params) => {
                let _ = events.send(SandboxEnvironmentEvent::Exit {
                    channel_id: params.channel_id,
                    code: params.code,
                });
            }
            Err(error) => {
                let _ = events.send(SandboxEnvironmentEvent::Error {
                    channel_id: None,
                    message: format!("Invalid channel.exit payload: {error}"),
                });
            }
        },
        SESSION_ERROR_METHOD => match request.parse_params::<SessionErrorParams>() {
            Ok(params) => {
                let _ = events.send(SandboxEnvironmentEvent::Error {
                    channel_id: params.channel_id,
                    message: params.message,
                });
            }
            Err(error) => {
                let _ = events.send(SandboxEnvironmentEvent::Error {
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
                let _ = events.send(SandboxEnvironmentEvent::Error {
                    channel_id: None,
                    message: format!("Unsupported connector message '{method}'."),
                });
                return Err(());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{render_launch_command, shell_single_quote};

    #[test]
    fn shell_single_quote_escapes_single_quotes() {
        assert_eq!(shell_single_quote("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn render_launch_command_quotes_shell_arguments() {
        let command = render_launch_command(
            "/mnt/data/sandbox-environment.tar.gz",
            "/app/ade/bin/reverse-connect",
            "wss://example.test/api/internal/reverse-connect/channel?id=1&x=2",
            "1712016000000.abc123def456",
            30,
        );

        assert_eq!(
            command,
            concat!(
                "set -eu\n",
                "tar --keep-directory-symlink -xzf '/mnt/data/sandbox-environment.tar.gz' -C /\n",
                "chmod 755 '/app/ade/bin/reverse-connect'\n",
                "exec '/app/ade/bin/reverse-connect' connect ",
                "--url 'wss://example.test/api/internal/reverse-connect/channel?id=1&x=2' ",
                "--bearer-token '1712016000000.abc123def456' ",
                "--idle-timeout-seconds 30"
            )
        );
    }
}
