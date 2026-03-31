use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::extract::ws::{Message, WebSocket};
use reqwest::Url;
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
    session::{ScopeSessionId, SessionExecution, SessionRuntimeArtifacts, SessionService},
    unix_time_ms,
};

use super::{
    launch::{SessionAgentLaunchConfig, encode_launch_config, render_launch_command},
    protocol::{SessionAgentCommand, SessionAgentEvent},
    rendezvous::{
        PendingSessionAgentManager, create_rendezvous_token, generate_channel_id,
        verify_rendezvous_token,
    },
};

const APP_URL_ENV_NAME: &str = "ADE_APP_URL";
const AGENT_BRIDGE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const AGENT_IDLE_SHUTDOWN_SECONDS: u64 = 15;
const AGENT_RENDEZVOUS_TOKEN_TTL_MS: u64 = 60_000;
const AGENT_EXECUTION_TIMEOUT_SECONDS: u64 = 3_600;

#[derive(Clone)]
pub struct ScopeSessionHandle {
    commands: mpsc::Sender<SessionAgentCommand>,
    events: broadcast::Sender<SessionAgentEvent>,
    prepare_lock: Arc<tokio::sync::Mutex<()>>,
    prepared: Arc<tokio::sync::Mutex<Option<PreparedRuntime>>>,
}

impl ScopeSessionHandle {
    pub fn is_closed(&self) -> bool {
        self.commands.is_closed()
    }

    pub async fn send(&self, command: SessionAgentCommand) -> Result<(), AppError> {
        self.commands.send(command).await.map_err(|_| {
            AppError::status(
                axum::http::StatusCode::BAD_GATEWAY,
                "Scope session agent is unavailable.",
            )
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionAgentEvent> {
        self.events.subscribe()
    }
}

#[derive(Clone)]
pub struct SessionAgentService {
    app_url: Url,
    manager: PendingSessionAgentManager,
    sessions: Arc<Mutex<HashMap<ScopeSessionId, SessionEntry>>>,
    session_secret: String,
    session_service: Arc<SessionService>,
}

#[derive(Clone)]
struct PreparedRuntime {
    config_version: String,
    engine_version: String,
    python_toolchain_version: String,
}

enum SessionEntry {
    Ready(ScopeSessionHandle),
    Starting(Vec<oneshot::Sender<Result<ScopeSessionHandle, String>>>),
}

impl SessionAgentService {
    pub fn from_env(env: &EnvBag, session_service: Arc<SessionService>) -> Result<Self, AppError> {
        let app_url = read_optional_trimmed_string(env, APP_URL_ENV_NAME).ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {APP_URL_ENV_NAME}"
            ))
        })?;
        let app_url = Url::parse(&app_url).map_err(|error| {
            AppError::config_with_source("ADE_APP_URL is not a valid URL.".to_string(), error)
        })?;
        match app_url.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(AppError::config(
                    "ADE_APP_URL must use http or https.".to_string(),
                ));
            }
        }

        Ok(Self {
            app_url,
            manager: PendingSessionAgentManager::default(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            session_secret: session_service.session_secret().to_string(),
            session_service,
        })
    }

    pub(crate) fn runtime_artifacts(
        &self,
        scope: &Scope,
    ) -> Result<SessionRuntimeArtifacts, AppError> {
        self.session_service.runtime_artifacts(scope)
    }

    pub(crate) fn claim_rendezvous(
        &self,
        channel_id: &str,
        token: &str,
    ) -> Result<oneshot::Sender<WebSocket>, AppError> {
        verify_rendezvous_token(&self.session_secret, channel_id, token, unix_time_ms())?;
        self.manager.claim(channel_id)
    }

    pub(crate) async fn connect_scope_session(
        &self,
        scope: &Scope,
        runtime: &SessionRuntimeArtifacts,
    ) -> Result<ScopeSessionHandle, AppError> {
        let scope_session_id = self.session_service.scope_session_id(scope);

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
                let result = self.launch_scope_session(scope, runtime).await;
                let mut waiters = Vec::new();
                {
                    let mut sessions = self.sessions.lock().expect("scope session lock poisoned");
                    match sessions.remove(&scope_session_id) {
                        Some(SessionEntry::Starting(pending)) => {
                            waiters = pending;
                        }
                        Some(SessionEntry::Ready(handle)) => {
                            return Ok(handle);
                        }
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

    pub(crate) async fn ensure_prepared(
        &self,
        scope: &Scope,
        runtime: &SessionRuntimeArtifacts,
        handle: &ScopeSessionHandle,
    ) -> Result<(), AppError> {
        let _guard = handle.prepare_lock.lock().await;
        {
            let prepared = handle.prepared.lock().await;
            if prepared.as_ref().is_some_and(|current| {
                current.config_version == runtime.config_version
                    && current.engine_version == runtime.engine_version
                    && current.python_toolchain_version == runtime.python_toolchain_version
            }) {
                return Ok(());
            }
        }

        self.upload_prepare_artifacts(scope, runtime).await?;

        let mut events = handle.subscribe();
        handle
            .send(SessionAgentCommand::Prepare {
                config_package_name: runtime.config_package_name.to_string(),
                config_version: runtime.config_version.clone(),
                config_wheel_path: runtime.config_wheel_path.clone(),
                engine_package_name: runtime.engine_package_name.to_string(),
                engine_version: runtime.engine_version.clone(),
                engine_wheel_path: runtime.engine_wheel_path.clone(),
                python_executable_path: runtime.python_executable_path.clone(),
                python_home_path: runtime.python_home_path.clone(),
                python_toolchain_path: runtime.python_toolchain_path.clone(),
                python_toolchain_version: runtime.python_toolchain_version.clone(),
            })
            .await?;

        let timeout = tokio::time::sleep(AGENT_BRIDGE_READY_TIMEOUT);
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(SessionAgentEvent::Prepared { config_version, engine_version, python_toolchain_version }) => {
                            let mut prepared = handle.prepared.lock().await;
                            *prepared = Some(PreparedRuntime {
                                config_version,
                                engine_version,
                                python_toolchain_version,
                            });
                            return Ok(());
                        }
                        Ok(SessionAgentEvent::Error { worker_id: None, message, .. }) => {
                            return Err(AppError::status(axum::http::StatusCode::BAD_GATEWAY, message));
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            return Err(AppError::status(
                                axum::http::StatusCode::BAD_GATEWAY,
                                "Scope session agent event stream overflowed during prepare.",
                            ));
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(AppError::status(
                                axum::http::StatusCode::BAD_GATEWAY,
                                "Scope session agent disconnected during prepare.",
                            ));
                        }
                    }
                }
                _ = &mut timeout => {
                    return Err(AppError::status(
                        axum::http::StatusCode::BAD_GATEWAY,
                        "Timed out waiting for the scope session to prepare.",
                    ));
                }
            }
        }
    }

    async fn upload_prepare_artifacts(
        &self,
        scope: &Scope,
        runtime: &SessionRuntimeArtifacts,
    ) -> Result<(), AppError> {
        self.session_service
            .upload_session_file(
                scope,
                &runtime.python_toolchain_filename,
                Some("application/gzip".to_string()),
                runtime.python_toolchain_bytes.clone(),
            )
            .await?;
        self.session_service
            .upload_session_file(
                scope,
                &runtime.engine_filename,
                Some("application/octet-stream".to_string()),
                runtime.engine_wheel_bytes.clone(),
            )
            .await?;
        self.session_service
            .upload_session_file(
                scope,
                &runtime.config_filename,
                Some("application/octet-stream".to_string()),
                runtime.config_wheel_bytes.clone(),
            )
            .await?;
        Ok(())
    }

    async fn launch_scope_session(
        &self,
        scope: &Scope,
        runtime: &SessionRuntimeArtifacts,
    ) -> Result<ScopeSessionHandle, AppError> {
        self.session_service
            .upload_session_file(
                scope,
                &runtime.agent_binary_filename,
                Some("application/octet-stream".to_string()),
                runtime.agent_binary_bytes.clone(),
            )
            .await?;

        let channel_id = generate_channel_id(&self.session_secret);
        let token = create_rendezvous_token(
            &self.session_secret,
            &channel_id,
            unix_time_ms() + AGENT_RENDEZVOUS_TOKEN_TTL_MS,
        );
        let bridge_rx = self.manager.create(channel_id.clone());
        let mut bridge_url = self.app_url.clone();
        let scheme = if bridge_url.scheme() == "http" {
            "ws"
        } else {
            "wss"
        };
        bridge_url
            .set_scheme(scheme)
            .expect("ADE_APP_URL scheme was validated at startup");
        bridge_url.set_path(&format!("/api/internal/session-agents/{channel_id}"));
        bridge_url.set_query(None);
        bridge_url.query_pairs_mut().append_pair("token", &token);

        let launch_config = encode_launch_config(&SessionAgentLaunchConfig {
            bridge_url: bridge_url.to_string(),
            idle_shutdown_seconds: AGENT_IDLE_SHUTDOWN_SECONDS,
        })?;
        self.session_service
            .upload_session_file(
                scope,
                &runtime.agent_launch_config_filename,
                Some("application/json".to_string()),
                launch_config,
            )
            .await?;

        let command = render_launch_command(
            &runtime.agent_binary_path,
            &runtime.agent_launch_config_path,
        );

        let session_service = Arc::clone(&self.session_service);
        let scope_for_execution = scope.clone();
        let mut execution_task = tokio::spawn(async move {
            session_service
                .execute_shell_detailed(
                    &scope_for_execution,
                    command,
                    Some(AGENT_EXECUTION_TIMEOUT_SECONDS),
                )
                .await
                .map(|result| result.value)
        });

        let bridge_socket = self
            .wait_for_bridge_socket(&channel_id, bridge_rx, &mut execution_task)
            .await?;

        let (events_tx, _) = broadcast::channel(256);
        let (command_tx, command_rx) = mpsc::channel(256);
        let handle = ScopeSessionHandle {
            commands: command_tx,
            events: events_tx.clone(),
            prepare_lock: Arc::new(tokio::sync::Mutex::new(())),
            prepared: Arc::new(tokio::sync::Mutex::new(None)),
        };
        let mut ready_rx = events_tx.subscribe();
        tokio::spawn(run_scope_session_task(
            bridge_socket,
            command_rx,
            events_tx,
            execution_task,
        ));
        wait_for_agent_ready(&mut ready_rx).await?;
        Ok(handle)
    }

    async fn wait_for_bridge_socket(
        &self,
        channel_id: &str,
        bridge_rx: oneshot::Receiver<WebSocket>,
        execution_task: &mut JoinHandle<Result<SessionExecution, AppError>>,
    ) -> Result<WebSocket, AppError> {
        let startup_timeout = tokio::time::sleep(AGENT_BRIDGE_READY_TIMEOUT);
        tokio::pin!(startup_timeout);
        tokio::pin!(bridge_rx);

        let result = tokio::select! {
            result = &mut bridge_rx => {
                result.map_err(|_| AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Scope session agent cancelled before the control channel connected.",
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
                    "Timed out waiting for the scope session agent to connect.",
                ))
            }
        };

        if result.is_err() && !execution_task.is_finished() {
            execution_task.abort();
            let _ = execution_task.await;
        }

        result
    }
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
                "Scope session agent exited before the control channel connected.".to_string()
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
                    "Scope session agent exited with status {}.",
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

async fn wait_for_agent_ready(
    ready_rx: &mut broadcast::Receiver<SessionAgentEvent>,
) -> Result<(), AppError> {
    let timeout = tokio::time::sleep(AGENT_BRIDGE_READY_TIMEOUT);
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            event = ready_rx.recv() => {
                match event {
                    Ok(SessionAgentEvent::Ready) => return Ok(()),
                    Ok(SessionAgentEvent::Error { message, .. }) => {
                        return Err(AppError::status(axum::http::StatusCode::BAD_GATEWAY, message));
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        return Err(AppError::status(
                            axum::http::StatusCode::BAD_GATEWAY,
                            "Scope session agent event stream overflowed before startup completed.",
                        ));
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(AppError::status(
                            axum::http::StatusCode::BAD_GATEWAY,
                            "Scope session agent closed before becoming ready.",
                        ));
                    }
                }
            }
            _ = &mut timeout => {
                return Err(AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Timed out waiting for the scope session agent to become ready.",
                ));
            }
        }
    }
}

async fn run_scope_session_task(
    mut socket: WebSocket,
    mut commands: mpsc::Receiver<SessionAgentCommand>,
    events: broadcast::Sender<SessionAgentEvent>,
    mut execution_task: JoinHandle<Result<SessionExecution, AppError>>,
) {
    loop {
        tokio::select! {
            maybe_command = commands.recv() => {
                let Some(command) = maybe_command else {
                    break;
                };
                let Ok(payload) = serde_json::to_string(&command) else {
                    let _ = events.send(SessionAgentEvent::Error {
                        message: "Failed to encode a scope session command.".to_string(),
                        phase: None,
                        retriable: false,
                        worker_id: None,
                    });
                    break;
                };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    let _ = events.send(SessionAgentEvent::Error {
                        message: "Scope session agent disconnected.".to_string(),
                        phase: None,
                        retriable: true,
                        worker_id: None,
                    });
                    break;
                }
            }
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<SessionAgentEvent>(text.as_str()) {
                            Ok(event) => {
                                let _ = events.send(event);
                            }
                            Err(error) => {
                                let _ = events.send(SessionAgentEvent::Error {
                                    message: format!("Invalid scope session event: {error}"),
                                    phase: None,
                                    retriable: false,
                                    worker_id: None,
                                });
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = events.send(SessionAgentEvent::Error {
                            message: "Scope session agent disconnected.".to_string(),
                            phase: None,
                            retriable: true,
                            worker_id: None,
                        });
                        break;
                    }
                    Some(Ok(Message::Binary(_))) => {
                        let _ = events.send(SessionAgentEvent::Error {
                            message: "Binary scope session messages are not supported.".to_string(),
                            phase: None,
                            retriable: false,
                            worker_id: None,
                        });
                        break;
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Err(error)) => {
                        let _ = events.send(SessionAgentEvent::Error {
                            message: format!("Scope session websocket failed: {error}"),
                            phase: None,
                            retriable: true,
                            worker_id: None,
                        });
                        break;
                    }
                }
            }
            result = &mut execution_task => {
                match result {
                    Ok(Ok(execution)) if matches!(execution.status.as_str(), "Success" | "Succeeded" | "0") => {}
                    Ok(Ok(execution)) => {
                        let message = if execution.stderr.trim().is_empty() {
                            execution.stdout.trim().to_string()
                        } else {
                            execution.stderr.trim().to_string()
                        };
                        let _ = events.send(SessionAgentEvent::Error {
                            message: if message.is_empty() {
                                format!("Scope session agent exited with status {}.", execution.status)
                            } else {
                                message
                            },
                            phase: None,
                            retriable: true,
                            worker_id: None,
                        });
                    }
                    Ok(Err(error)) => {
                        let _ = events.send(SessionAgentEvent::Error {
                            message: error.to_string(),
                            phase: None,
                            retriable: true,
                            worker_id: None,
                        });
                    }
                    Err(error) => {
                        let _ = events.send(SessionAgentEvent::Error {
                            message: format!("Scope session execution task failed: {error}"),
                            phase: None,
                            retriable: true,
                            worker_id: None,
                        });
                    }
                }
                break;
            }
        }
    }

    let _ = socket.send(Message::Close(None)).await;
    if !execution_task.is_finished() {
        execution_task.abort();
        let _ = execution_task.await;
    }
}
