use std::{
    collections::HashSet,
    ops::ControlFlow,
    sync::{Arc, Mutex},
};

use axum::extract::ws::{Message, WebSocket};
use uuid::Uuid;

use crate::{
    config::EnvBag,
    error::AppError,
    scope::Scope,
    session_agent::{
        SessionAgentCommand, SessionAgentEvent, SessionAgentService, WorkerId, WorkerKind,
    },
};

use super::protocol::{
    TerminalClientMessage, TerminalServerMessage, parse_client_message, send_terminal_event,
};

const DEFAULT_TERMINAL_COLS: u16 = 120;
const DEFAULT_TERMINAL_ROWS: u16 = 32;

#[derive(Clone)]
pub struct TerminalService {
    active_sessions: ActiveTerminalManager,
    session_agent_service: Arc<SessionAgentService>,
}

impl TerminalService {
    pub fn from_env(
        _env: &EnvBag,
        session_agent_service: Arc<SessionAgentService>,
    ) -> Result<Self, AppError> {
        Ok(Self {
            active_sessions: ActiveTerminalManager::default(),
            session_agent_service,
        })
    }

    pub(crate) async fn run_browser_terminal(&self, scope: Scope, mut browser_socket: WebSocket) {
        let Some(_lease) = self.active_sessions.try_acquire(&scope) else {
            let _ = send_terminal_event(
                    &mut browser_socket,
                    TerminalServerMessage::Error {
                        message: "A terminal session for this workspace is still shutting down. Retry in a few seconds.".to_string(),
                    },
                )
                .await;
            let _ = browser_socket.send(Message::Close(None)).await;
            return;
        };

        let runtime = match self.session_agent_service.runtime_artifacts(&scope) {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = Self::fail_terminal(&mut browser_socket, error.to_string(), None).await;
                return;
            }
        };

        let handle = match self
            .session_agent_service
            .connect_scope_session(&scope, &runtime)
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                let _ = Self::fail_terminal(&mut browser_socket, error.to_string(), None).await;
                return;
            }
        };

        if let Err(error) = self
            .session_agent_service
            .ensure_prepared(&scope, &runtime, &handle)
            .await
        {
            let _ = Self::fail_terminal(&mut browser_socket, error.to_string(), None).await;
            return;
        }

        let worker_id = WorkerId::new(format!("shell-{}", Uuid::new_v4().simple()));
        let mut events = handle.subscribe();
        if let Err(error) = handle
            .send(SessionAgentCommand::StartShell {
                cols: DEFAULT_TERMINAL_COLS,
                cwd: runtime.session_root.clone(),
                rows: DEFAULT_TERMINAL_ROWS,
                worker_id: worker_id.clone(),
            })
            .await
        {
            let _ = Self::fail_terminal(&mut browser_socket, error.to_string(), None).await;
            return;
        }

        if send_terminal_event(&mut browser_socket, TerminalServerMessage::Ready)
            .await
            .is_err()
        {
            let _ = handle
                .send(SessionAgentCommand::CloseWorker {
                    worker_id: worker_id.clone(),
                })
                .await;
            let _ = browser_socket.send(Message::Close(None)).await;
            return;
        }

        loop {
            tokio::select! {
                browser_message = browser_socket.recv() => {
                    match Self::handle_browser_message(browser_message, &handle, &worker_id)
                        .await
                    {
                        Ok(ControlFlow::Continue(())) => {}
                        Ok(ControlFlow::Break(())) => break,
                        Err(error) => {
                            let _ = Self::fail_terminal(&mut browser_socket, error.to_string(), None).await;
                            break;
                        }
                    }
                }
                event = events.recv() => {
                    match Self::handle_agent_event(event, &worker_id, &mut browser_socket)
                        .await
                    {
                        Ok(ControlFlow::Continue(())) => {}
                        Ok(ControlFlow::Break(())) => break,
                        Err(error) => {
                            let _ = Self::fail_terminal(&mut browser_socket, error.to_string(), None).await;
                            break;
                        }
                    }
                }
            }
        }

        let _ = handle
            .send(SessionAgentCommand::CloseWorker {
                worker_id: worker_id.clone(),
            })
            .await;
        let _ = browser_socket.send(Message::Close(None)).await;
    }

    async fn handle_browser_message(
        message: Option<Result<Message, axum::Error>>,
        handle: &crate::session_agent::ScopeSessionHandle,
        worker_id: &WorkerId,
    ) -> Result<ControlFlow<()>, AppError> {
        match message {
            Some(Ok(Message::Text(text))) => match parse_client_message(text.as_str())? {
                TerminalClientMessage::Close => {
                    let _ = handle
                        .send(SessionAgentCommand::CloseWorker {
                            worker_id: worker_id.clone(),
                        })
                        .await;
                    Ok(ControlFlow::Break(()))
                }
                TerminalClientMessage::Input { data } => {
                    handle
                        .send(SessionAgentCommand::WriteInput {
                            data,
                            worker_id: worker_id.clone(),
                        })
                        .await?;
                    Ok(ControlFlow::Continue(()))
                }
                TerminalClientMessage::Resize { cols, rows } => {
                    handle
                        .send(SessionAgentCommand::ResizePty {
                            cols,
                            rows,
                            worker_id: worker_id.clone(),
                        })
                        .await?;
                    Ok(ControlFlow::Continue(()))
                }
            },
            Some(Ok(Message::Binary(_))) => Err(AppError::request(
                "Binary terminal messages are not supported.".to_string(),
            )),
            Some(Ok(Message::Close(_))) | None => {
                let _ = handle
                    .send(SessionAgentCommand::CloseWorker {
                        worker_id: worker_id.clone(),
                    })
                    .await;
                Ok(ControlFlow::Break(()))
            }
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                Ok(ControlFlow::Continue(()))
            }
            Some(Err(error)) => Err(AppError::internal_with_source(
                "Browser websocket failed.",
                error,
            )),
        }
    }

    async fn handle_agent_event(
        event: Result<SessionAgentEvent, tokio::sync::broadcast::error::RecvError>,
        worker_id: &WorkerId,
        browser_socket: &mut WebSocket,
    ) -> Result<ControlFlow<()>, AppError> {
        match event {
            Ok(SessionAgentEvent::PtyOutput {
                worker_id: event_worker_id,
                data,
            }) if event_worker_id == *worker_id => {
                send_terminal_event(browser_socket, TerminalServerMessage::Output { data }).await?;
                Ok(ControlFlow::Continue(()))
            }
            Ok(SessionAgentEvent::WorkerExit {
                worker_id: event_worker_id,
                kind: WorkerKind::Shell,
                code,
                ..
            }) if event_worker_id == *worker_id => {
                send_terminal_event(browser_socket, TerminalServerMessage::Exit { code }).await?;
                Ok(ControlFlow::Break(()))
            }
            Ok(SessionAgentEvent::Error {
                worker_id: Some(event_worker_id),
                message,
                ..
            }) if event_worker_id == *worker_id => {
                Self::fail_terminal(browser_socket, message, None).await?;
                Ok(ControlFlow::Break(()))
            }
            Ok(SessionAgentEvent::Error {
                worker_id: None,
                message,
                ..
            }) => {
                Self::fail_terminal(browser_socket, message, None).await?;
                Ok(ControlFlow::Break(()))
            }
            Ok(_) => Ok(ControlFlow::Continue(())),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                Self::fail_terminal(
                    browser_socket,
                    "Scope session agent event stream overflowed while the terminal was active."
                        .to_string(),
                    None,
                )
                .await?;
                Ok(ControlFlow::Break(()))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                Self::fail_terminal(
                    browser_socket,
                    "Scope session agent disconnected while the terminal was active.".to_string(),
                    None,
                )
                .await?;
                Ok(ControlFlow::Break(()))
            }
        }
    }

    async fn fail_terminal(
        browser_socket: &mut WebSocket,
        message: String,
        code: Option<i32>,
    ) -> Result<(), AppError> {
        send_terminal_event(browser_socket, TerminalServerMessage::Error { message }).await?;
        send_terminal_event(browser_socket, TerminalServerMessage::Exit { code }).await
    }
}

#[derive(Default, Clone)]
struct ActiveTerminalManager {
    inner: Arc<Mutex<HashSet<Scope>>>,
}

impl ActiveTerminalManager {
    fn try_acquire(&self, scope: &Scope) -> Option<ActiveTerminalLease> {
        let mut active = self
            .inner
            .lock()
            .expect("active terminal session lock poisoned");

        if !active.insert(scope.clone()) {
            return None;
        }

        Some(ActiveTerminalLease {
            scope: scope.clone(),
            manager: self.clone(),
        })
    }
}

struct ActiveTerminalLease {
    scope: Scope,
    manager: ActiveTerminalManager,
}

impl Drop for ActiveTerminalLease {
    fn drop(&mut self) {
        let _ = self
            .manager
            .inner
            .lock()
            .expect("active terminal session lock poisoned")
            .remove(&self.scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_service_allows_only_one_active_scope_lease() {
        let manager = ActiveTerminalManager::default();
        let scope = Scope {
            workspace_id: "workspace-a".to_string(),
            config_version_id: "config-v1".to_string(),
        };

        let lease = manager.try_acquire(&scope).expect("first lease");
        assert!(manager.try_acquire(&scope).is_none());
        drop(lease);
        assert!(manager.try_acquire(&scope).is_some());
    }
}
