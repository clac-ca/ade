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
    scope_session::{
        ChannelId, ChannelKind, ChannelOpenParams, ChannelStream, PtySize, ScopeSessionEvent,
        ScopeSessionService,
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
    scope_session_service: Arc<ScopeSessionService>,
}

impl TerminalService {
    pub fn from_env(
        _env: &EnvBag,
        scope_session_service: Arc<ScopeSessionService>,
    ) -> Result<Self, AppError> {
        Ok(Self {
            active_sessions: ActiveTerminalManager::default(),
            scope_session_service,
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

        let handle = match self
            .scope_session_service
            .ensure_ready_scope_session(&scope)
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                let _ = Self::fail_terminal(&mut browser_socket, error.to_string(), None).await;
                return;
            }
        };

        let channel_id = ChannelId::new(format!("pty-{}", Uuid::new_v4().simple()));
        let mut events = handle.subscribe();
        if let Err(error) = handle
            .open_channel(ChannelOpenParams {
                channel_id: channel_id.clone(),
                command: "exec /bin/sh -i".to_string(),
                cwd: Some(handle.session_root().to_string()),
                env: Default::default(),
                kind: ChannelKind::Pty,
                pty: Some(PtySize {
                    cols: DEFAULT_TERMINAL_COLS,
                    rows: DEFAULT_TERMINAL_ROWS,
                }),
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
            let _ = handle.close_channel(channel_id.clone()).await;
            let _ = browser_socket.send(Message::Close(None)).await;
            return;
        }

        loop {
            tokio::select! {
                browser_message = browser_socket.recv() => {
                    match Self::handle_browser_message(browser_message, &handle, &channel_id).await {
                        Ok(ControlFlow::Continue(())) => {}
                        Ok(ControlFlow::Break(())) => break,
                        Err(error) => {
                            let _ = Self::fail_terminal(&mut browser_socket, error.to_string(), None).await;
                            break;
                        }
                    }
                }
                event = events.recv() => {
                    match Self::handle_connector_event(event, &channel_id, &mut browser_socket).await {
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

        let _ = handle.close_channel(channel_id.clone()).await;
        let _ = browser_socket.send(Message::Close(None)).await;
    }

    async fn handle_browser_message(
        message: Option<Result<Message, axum::Error>>,
        handle: &crate::scope_session::ScopeSession,
        channel_id: &ChannelId,
    ) -> Result<ControlFlow<()>, AppError> {
        match message {
            Some(Ok(Message::Text(text))) => match parse_client_message(text.as_str())? {
                TerminalClientMessage::Close => {
                    let _ = handle.close_channel(channel_id.clone()).await;
                    Ok(ControlFlow::Break(()))
                }
                TerminalClientMessage::Input { data } => {
                    handle
                        .send_stdin(channel_id.clone(), data.into_bytes())
                        .await?;
                    Ok(ControlFlow::Continue(()))
                }
                TerminalClientMessage::Resize { cols, rows } => {
                    handle.resize_pty(channel_id.clone(), cols, rows).await?;
                    Ok(ControlFlow::Continue(()))
                }
            },
            Some(Ok(Message::Binary(_))) => Err(AppError::request(
                "Binary terminal messages are not supported.".to_string(),
            )),
            Some(Ok(Message::Close(_))) | None => {
                let _ = handle.close_channel(channel_id.clone()).await;
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

    async fn handle_connector_event(
        event: Result<ScopeSessionEvent, tokio::sync::broadcast::error::RecvError>,
        channel_id: &ChannelId,
        browser_socket: &mut WebSocket,
    ) -> Result<ControlFlow<()>, AppError> {
        match event {
            Ok(ScopeSessionEvent::Data {
                channel_id: event_channel_id,
                data,
                stream: ChannelStream::Pty,
            }) if event_channel_id == *channel_id => {
                send_terminal_event(
                    browser_socket,
                    TerminalServerMessage::Output {
                        data: String::from_utf8_lossy(&data).into_owned(),
                    },
                )
                .await?;
                Ok(ControlFlow::Continue(()))
            }
            Ok(ScopeSessionEvent::Exit {
                channel_id: event_channel_id,
                code,
            }) if event_channel_id == *channel_id => {
                send_terminal_event(browser_socket, TerminalServerMessage::Exit { code }).await?;
                Ok(ControlFlow::Break(()))
            }
            Ok(ScopeSessionEvent::Error {
                channel_id: Some(event_channel_id),
                message,
            }) if event_channel_id == *channel_id => {
                Self::fail_terminal(browser_socket, message, None).await?;
                Ok(ControlFlow::Break(()))
            }
            Ok(ScopeSessionEvent::Error {
                channel_id: None,
                message,
            }) => {
                Self::fail_terminal(browser_socket, message, None).await?;
                Ok(ControlFlow::Break(()))
            }
            Ok(_) => Ok(ControlFlow::Continue(())),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                Self::fail_terminal(
                    browser_socket,
                    "Scope session connector event stream overflowed while the terminal was active."
                        .to_string(),
                    None,
                )
                .await?;
                Ok(ControlFlow::Break(()))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                Self::fail_terminal(
                    browser_socket,
                    "Scope session connector disconnected while the terminal was active."
                        .to_string(),
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
    fn terminal_manager_is_single_lease_per_scope() {
        let manager = ActiveTerminalManager::default();
        let scope = Scope {
            workspace_id: "workspace-a".to_string(),
            config_version_id: "config-v1".to_string(),
        };

        let first = manager.try_acquire(&scope);
        let second = manager.try_acquire(&scope);

        assert!(first.is_some());
        assert!(second.is_none());
    }
}
