use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::extract::ws::WebSocket;
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
    assets::{SandboxAssets, SandboxId},
    connector::{
        OutboundRpc, SandboxEnvironmentEvent, run_sandbox_environment_task,
        wait_for_connector_ready,
    },
    provider::SandboxProvider,
    rendezvous::{
        PendingConnectorManager, create_rendezvous_token, generate_channel_id,
        verify_rendezvous_token,
    },
};
use ::reverse_connect::protocol::{
    CHANNEL_CLOSE_METHOD, CHANNEL_OPEN_METHOD, CHANNEL_RESIZE_METHOD, CHANNEL_SIGNAL_METHOD,
    CHANNEL_STDIN_METHOD, ChannelCloseParams, ChannelId, ChannelOpenParams, ChannelResizeParams,
    ChannelSignalParams, ChannelStdinParams, ChannelStream, SignalName,
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
            SandboxAssets::from_paths(environment_archive, &sandbox_env)?,
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
        self.install_config(scope, handle).await
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
            )
            .await?;

        if output.code != Some(0) {
            return Err(AppError::status(
                axum::http::StatusCode::BAD_GATEWAY,
                captured_exec_failure_message("setup", output.code, &output.stdout, &output.stderr),
            ));
        }

        runtime_state.prepared_environment_revision =
            Some(self.assets.environment_revision().to_string());
        Ok(())
    }

    async fn install_config(
        &self,
        scope: &Scope,
        handle: &SandboxEnvironment,
    ) -> Result<(), AppError> {
        let config_directory = self.assets.config_mount_directory(scope);

        let output = self
            .run_exec_command(
                handle,
                "config",
                render_config_install_command(
                    handle.python_executable_path(),
                    self.assets.base_wheelhouse_path(),
                    &config_directory,
                ),
            )
            .await?;

        if output.code != Some(0) {
            return Err(AppError::status(
                axum::http::StatusCode::BAD_GATEWAY,
                captured_exec_failure_message(
                    "config installation",
                    output.code,
                    &output.stdout,
                    &output.stderr,
                ),
            ));
        }

        Ok(())
    }

    async fn run_exec_command(
        &self,
        handle: &SandboxEnvironment,
        channel_prefix: &str,
        command: String,
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
                                format!(
                                    "Sandbox environment event stream overflowed during {channel_prefix}."
                                ),
                            ));
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(AppError::status(
                                axum::http::StatusCode::BAD_GATEWAY,
                                format!(
                                    "Sandbox environment disconnected during {channel_prefix}."
                                ),
                            ));
                        }
                    }
                }
                _ = &mut timeout => {
                    let _ = handle.close_channel(channel_id.clone()).await;
                    return Err(AppError::status(
                        axum::http::StatusCode::BAD_GATEWAY,
                        format!("Timed out waiting for {channel_prefix} to finish."),
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

fn render_config_install_command(
    python_executable_path: &str,
    base_wheelhouse_path: &str,
    config_directory: &str,
) -> String {
    let python_executable_path = shell_single_quote(python_executable_path);
    let base_wheelhouse_path = shell_single_quote(base_wheelhouse_path);
    let config_glob = format!("{}/*.whl", shell_single_quote(config_directory));
    format!(
        "set -eu\nset -- {config_glob}\nif [ ! -f \"$1\" ]; then\n  echo \"No config wheel was mounted for this scope.\" >&2\n  exit 1\nfi\nexec {python_executable_path} -m pip install --upgrade --no-index --find-links {base_wheelhouse_path} \"$@\""
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

fn captured_exec_failure_message(
    operation: &str,
    code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    if !stdout.is_empty() {
        return stdout;
    }
    format!(
        "Sandbox environment {operation} exited with code {}.",
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

#[cfg(test)]
mod tests {
    use super::{render_config_install_command, render_launch_command, shell_single_quote};

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

    #[test]
    fn render_config_install_command_uses_mounted_scope_directory() {
        let command = render_config_install_command(
            "/app/ade/python/current/bin/python3",
            "/app/ade/wheelhouse/base",
            "/mnt/data/ade/configs/workspace-a/config-v1",
        );

        assert_eq!(
            command,
            concat!(
                "set -eu\n",
                "set -- '/mnt/data/ade/configs/workspace-a/config-v1'/*.whl\n",
                "if [ ! -f \"$1\" ]; then\n",
                "  echo \"No config wheel was mounted for this scope.\" >&2\n",
                "  exit 1\n",
                "fi\n",
                "exec '/app/ade/python/current/bin/python3' -m pip install --upgrade --no-index ",
                "--find-links '/app/ade/wheelhouse/base' \"$@\""
            )
        );
    }
}
