use std::{
    collections::HashSet,
    ops::ControlFlow,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::extract::ws::{Message, WebSocket};
use reqwest::Url;
use tokio::{sync::oneshot, task::JoinHandle, time::Instant};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
    session::{PythonExecution, SessionService},
    unix_time_ms,
};

use super::{
    bootstrap::{TerminalBootstrapConfig, render_bootstrap_code},
    bridge::{
        PendingTerminalManager, create_bridge_token, generate_channel_id, verify_bridge_token,
    },
    protocol::{
        TerminalClientMessage, TerminalServerMessage, forward_bridge_message,
        forward_browser_message, parse_client_message, parse_server_message, send_close_message,
        send_terminal_event,
    },
};

const APP_URL_ENV_NAME: &str = "ADE_APP_URL";
const BRIDGE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const BRIDGE_TOKEN_TTL_MS: u64 = 60_000;
const DEFAULT_TERMINAL_COLS: u16 = 120;
const DEFAULT_TERMINAL_ROWS: u16 = 32;
const TERMINAL_EXECUTION_TIMEOUT_SECONDS: u64 = 220;

#[derive(Debug)]
enum TerminalPhase<T> {
    AwaitExecution,
    Continue(T),
    Finished,
}

#[derive(Clone)]
pub struct TerminalService {
    active_sessions: ActiveTerminalManager,
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
            active_sessions: ActiveTerminalManager::default(),
            app_url: parsed_app_url,
            manager: PendingTerminalManager::default(),
            session_secret: session_service.session_secret().to_string(),
            session_service,
        })
    }

    pub(crate) async fn run_browser_terminal(&self, scope: Scope, mut browser_socket: WebSocket) {
        let lease = match self.active_sessions.try_acquire(&scope) {
            Ok(lease) => lease,
            Err(_) => {
                let _ = send_terminal_event(
                    &mut browser_socket,
                    TerminalServerMessage::error(
                        "A terminal session for this workspace is still shutting down. Retry in a few seconds.".to_string(),
                    ),
                )
                .await;
                let _ = browser_socket.send(Message::Close(None)).await;
                return;
            }
        };

        let channel_id = generate_channel_id(&self.session_secret);
        let token = create_bridge_token(
            &self.session_secret,
            &channel_id,
            unix_time_ms() + BRIDGE_TOKEN_TTL_MS,
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
        bridge_url.set_path(&format!("/api/internal/terminals/{channel_id}"));
        bridge_url.set_query(None);
        bridge_url.query_pairs_mut().append_pair("token", &token);

        let bootstrap_code = match render_bootstrap_code(&TerminalBootstrapConfig {
            bridge_url: bridge_url.to_string(),
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
                self.manager.cancel(&channel_id);
                return;
            }
        };

        let session_service = Arc::clone(&self.session_service);
        let mut execution_task = tokio::spawn(async move {
            Ok(session_service
                .execute_inline_python_detailed(
                    &scope,
                    bootstrap_code,
                    Some(TERMINAL_EXECUTION_TIMEOUT_SECONDS),
                )
                .await?
                .value)
        });
        let startup_deadline = Instant::now() + BRIDGE_READY_TIMEOUT;

        let mut bridge_socket = match self
            .wait_for_bridge_socket(
                &channel_id,
                bridge_rx,
                &mut browser_socket,
                &mut execution_task,
                startup_deadline,
            )
            .await
        {
            TerminalPhase::Continue(socket) => socket,
            TerminalPhase::AwaitExecution => {
                spawn_execution_cleanup(execution_task, lease);
                return;
            }
            TerminalPhase::Finished => return,
        };

        match self
            .wait_for_ready_message(
                &mut browser_socket,
                &mut bridge_socket,
                &mut execution_task,
                startup_deadline,
            )
            .await
        {
            TerminalPhase::Continue(()) => {}
            TerminalPhase::AwaitExecution => {
                spawn_execution_cleanup(execution_task, lease);
                return;
            }
            TerminalPhase::Finished => return,
        }

        self.relay_terminal_session(browser_socket, bridge_socket, execution_task, lease)
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

    async fn wait_for_bridge_socket(
        &self,
        channel_id: &str,
        bridge_rx: oneshot::Receiver<WebSocket>,
        browser_socket: &mut WebSocket,
        execution_task: &mut JoinHandle<Result<PythonExecution, AppError>>,
        startup_deadline: Instant,
    ) -> TerminalPhase<WebSocket> {
        let startup_timeout = tokio::time::sleep_until(startup_deadline);
        tokio::pin!(startup_timeout);
        tokio::pin!(bridge_rx);

        loop {
            tokio::select! {
                bridge_result = &mut bridge_rx => {
                    return match bridge_result {
                        Ok(socket) => TerminalPhase::Continue(socket),
                        Err(_) => {
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(
                                    "Terminal startup was cancelled before the bridge connected.".to_string(),
                                ),
                            ).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            TerminalPhase::AwaitExecution
                        }
                    };
                }
                browser_message = browser_socket.recv() => {
                    match browser_message {
                        Some(Ok(Message::Text(text))) => match parse_client_message(text.as_str()) {
                            Ok(TerminalClientMessage::Close) => {
                                self.manager.cancel(channel_id);
                                return TerminalPhase::AwaitExecution;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                self.manager.cancel(channel_id);
                                let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(error.to_string())).await;
                                let _ = browser_socket.send(Message::Close(None)).await;
                                return TerminalPhase::AwaitExecution;
                            }
                        },
                        Some(Ok(Message::Binary(_))) => {
                            self.manager.cancel(channel_id);
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(
                                    "Binary terminal messages are not supported.".to_string(),
                                ),
                            ).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return TerminalPhase::AwaitExecution;
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            self.manager.cancel(channel_id);
                            return TerminalPhase::AwaitExecution;
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                        Some(Err(error)) => {
                            self.manager.cancel(channel_id);
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(format!("Browser websocket failed: {error}")),
                            ).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return TerminalPhase::AwaitExecution;
                        }
                    }
                }
                result = &mut *execution_task => {
                    self.manager.cancel(channel_id);
                    let message = match join_execution_result(result) {
                        Ok(execution) => execution_failure_message(&execution),
                        Err(error) => error.to_string(),
                    };
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(message)).await;
                    let _ = browser_socket.send(Message::Close(None)).await;
                    return TerminalPhase::Finished;
                }
                _ = &mut startup_timeout => {
                    self.manager.cancel(channel_id);
                    let _ = send_terminal_event(
                        browser_socket,
                        TerminalServerMessage::error(
                            "Timed out waiting for the terminal bridge to connect.".to_string(),
                        ),
                    ).await;
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                    let _ = browser_socket.send(Message::Close(None)).await;
                    return TerminalPhase::AwaitExecution;
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
    ) -> TerminalPhase<()> {
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
                                        return TerminalPhase::AwaitExecution;
                                    }
                                    return TerminalPhase::Continue(());
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
                                    return TerminalPhase::AwaitExecution;
                                }
                                Err(error) => {
                                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(error.to_string())).await;
                                    let _ = send_close_message(bridge_socket).await;
                                    let _ = browser_socket.send(Message::Close(None)).await;
                                    return TerminalPhase::AwaitExecution;
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
                            return TerminalPhase::AwaitExecution;
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Close(_))) | None => {
                            let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return TerminalPhase::AwaitExecution;
                        }
                        Some(Err(error)) => {
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(format!("Terminal bridge failed: {error}")),
                            ).await;
                            let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return TerminalPhase::AwaitExecution;
                        }
                    }
                }
                browser_message = browser_socket.recv() => {
                    match browser_message {
                        Some(Ok(Message::Text(text))) => match parse_client_message(text.as_str()) {
                            Ok(TerminalClientMessage::Close) => {
                                let _ = send_close_message(bridge_socket).await;
                                return TerminalPhase::AwaitExecution;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(error.to_string())).await;
                                let _ = send_close_message(bridge_socket).await;
                                let _ = browser_socket.send(Message::Close(None)).await;
                                return TerminalPhase::AwaitExecution;
                            }
                        },
                        Some(Ok(Message::Binary(_))) => {
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(
                                    "Binary terminal messages are not supported.".to_string(),
                                ),
                            ).await;
                            let _ = send_close_message(bridge_socket).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return TerminalPhase::AwaitExecution;
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            let _ = send_close_message(bridge_socket).await;
                            return TerminalPhase::AwaitExecution;
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                        Some(Err(error)) => {
                            let _ = send_terminal_event(
                                browser_socket,
                                TerminalServerMessage::error(format!("Browser websocket failed: {error}")),
                            ).await;
                            let _ = send_close_message(bridge_socket).await;
                            let _ = browser_socket.send(Message::Close(None)).await;
                            return TerminalPhase::AwaitExecution;
                        }
                    }
                }
                result = &mut *execution_task => {
                    let message = match join_execution_result(result) {
                        Ok(execution) => execution_failure_message(&execution),
                        Err(error) => error.to_string(),
                    };
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::error(message)).await;
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                    let _ = send_close_message(bridge_socket).await;
                    let _ = browser_socket.send(Message::Close(None)).await;
                    return TerminalPhase::Finished;
                }
                _ = &mut startup_timeout => {
                    let _ = send_terminal_event(
                        browser_socket,
                        TerminalServerMessage::error(
                            "Timed out waiting for the terminal bridge to become ready.".to_string(),
                        ),
                    ).await;
                    let _ = send_terminal_event(browser_socket, TerminalServerMessage::exit(None)).await;
                    let _ = send_close_message(bridge_socket).await;
                    let _ = browser_socket.send(Message::Close(None)).await;
                    return TerminalPhase::AwaitExecution;
                }
            }
        }
    }

    async fn relay_terminal_session(
        &self,
        mut browser_socket: WebSocket,
        mut bridge_socket: WebSocket,
        mut execution_task: JoinHandle<Result<PythonExecution, AppError>>,
        lease: ActiveTerminalLease,
    ) {
        let session_timeout =
            tokio::time::sleep(Duration::from_secs(TERMINAL_EXECUTION_TIMEOUT_SECONDS));
        tokio::pin!(session_timeout);
        let mut phase = TerminalPhase::AwaitExecution;

        loop {
            tokio::select! {
                browser_message = browser_socket.recv() => {
                    match browser_message {
                        Some(Ok(message)) => {
                            match forward_browser_message(message, &mut bridge_socket).await {
                                Ok(ControlFlow::Continue(())) => {}
                                Ok(ControlFlow::Break(())) => break,
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
                                Ok(ControlFlow::Continue(())) => {}
                                Ok(ControlFlow::Break(())) => break,
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
                    match join_execution_result(result) {
                        Ok(execution) if matches!(execution.status.as_str(), "Success" | "Succeeded" | "0") => {}
                        Ok(execution) => {
                            let _ = send_terminal_event(
                                &mut browser_socket,
                                TerminalServerMessage::error(execution_failure_message(&execution)),
                            ).await;
                            let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::exit(None)).await;
                            let _ = send_close_message(&mut bridge_socket).await;
                            phase = TerminalPhase::Finished;
                            break;
                        }
                        Err(error) => {
                            let _ = send_terminal_event(
                                &mut browser_socket,
                                TerminalServerMessage::error(error.to_string()),
                            ).await;
                            let _ = send_terminal_event(&mut browser_socket, TerminalServerMessage::exit(None)).await;
                            let _ = send_close_message(&mut bridge_socket).await;
                            phase = TerminalPhase::Finished;
                            break;
                        }
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
        match phase {
            TerminalPhase::AwaitExecution => spawn_execution_cleanup(execution_task, lease),
            TerminalPhase::Finished => {}
            TerminalPhase::Continue(()) => {
                unreachable!("terminal relay does not continue after exit")
            }
        }
    }
}

#[derive(Default, Clone)]
struct ActiveTerminalManager {
    inner: Arc<Mutex<HashSet<String>>>,
}

impl ActiveTerminalManager {
    fn try_acquire(&self, scope: &Scope) -> Result<ActiveTerminalLease, ()> {
        let key = format!("{}:{}", scope.workspace_id, scope.config_version_id);
        let mut active = self
            .inner
            .lock()
            .expect("active terminal session lock poisoned");

        if !active.insert(key.clone()) {
            return Err(());
        }

        Ok(ActiveTerminalLease {
            key,
            manager: self.clone(),
        })
    }
}

struct ActiveTerminalLease {
    key: String,
    manager: ActiveTerminalManager,
}

impl Drop for ActiveTerminalLease {
    fn drop(&mut self) {
        let _ = self
            .manager
            .inner
            .lock()
            .expect("active terminal session lock poisoned")
            .remove(&self.key);
    }
}

fn spawn_execution_cleanup(
    execution_task: JoinHandle<Result<PythonExecution, AppError>>,
    lease: ActiveTerminalLease,
) {
    tokio::spawn(async move {
        let _ = execution_task.await;
        drop(lease);
    });
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

fn execution_failure_message(execution: &PythonExecution) -> String {
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
        assert!(code.contains("websockets.sync.client"));
        assert!(code.contains("codecs.getincrementaldecoder"));
        assert!(code.contains("wss://example.com/api/internal/terminals/channel"));
    }

    #[test]
    fn pending_bridges_are_removed_on_cancel_and_claim() {
        let manager = PendingTerminalManager::default();
        let _bridge_rx = manager.create("channel-a".to_string());

        manager.cancel("channel-a");
        assert!(manager.claim("channel-a").is_err());

        let _bridge_rx = manager.create("channel-b".to_string());
        let _attachment = manager.claim("channel-b").unwrap();
        assert!(manager.claim("channel-b").is_err());
    }
}
