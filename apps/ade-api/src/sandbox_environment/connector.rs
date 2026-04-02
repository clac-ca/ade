use std::{collections::HashMap, time::Duration};

use axum::extract::ws::{Message, WebSocket};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};

use crate::{error::AppError, session_pool::SessionExecution};

use ::reverse_connect::protocol::{
    CHANNEL_DATA_METHOD, CHANNEL_EXIT_METHOD, CONNECTOR_HELLO_METHOD, ChannelDataParams,
    ChannelExitParams, ConnectorHelloParams, EmptyResult, RequestMessage, ResponseMessage,
    RpcMessage, SESSION_ERROR_METHOD, SessionErrorParams,
};

use super::{ChannelId, ChannelStream};

const CONNECTOR_READY_TIMEOUT: Duration = Duration::from_secs(45);

const CONNECTOR_READY_TIMEOUT_MESSAGE: &str =
    "Timed out waiting for the sandbox environment connector to become ready.";
const CONNECTOR_STREAM_OVERFLOW_MESSAGE: &str =
    "Sandbox environment event stream overflowed before startup completed.";
const CONNECTOR_CLOSED_MESSAGE: &str =
    "Sandbox environment connector closed before becoming ready.";

pub(super) enum OutboundRpc {
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

pub(super) async fn wait_for_connector_ready(
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
                            CONNECTOR_STREAM_OVERFLOW_MESSAGE,
                        ));
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(AppError::status(
                            axum::http::StatusCode::BAD_GATEWAY,
                            CONNECTOR_CLOSED_MESSAGE,
                        ));
                    }
                }
            }
            _ = &mut timeout => {
                return Err(AppError::status(
                    axum::http::StatusCode::BAD_GATEWAY,
                    CONNECTOR_READY_TIMEOUT_MESSAGE,
                ));
            }
        }
    }
}

pub(super) async fn run_sandbox_environment_task(
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
