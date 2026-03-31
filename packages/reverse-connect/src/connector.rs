use std::{
    collections::HashMap,
    io::{Read, Write},
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use http::{
    HeaderValue,
    header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL},
};
use portable_pty::{CommandBuilder, PtySize as PortablePtySize, native_pty_system};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use crate::protocol::{
    CHANNEL_CLOSE_METHOD, CHANNEL_DATA_METHOD, CHANNEL_EXIT_METHOD, CHANNEL_OPEN_METHOD,
    CHANNEL_RESIZE_METHOD, CHANNEL_SIGNAL_METHOD, CHANNEL_STDIN_METHOD, CONNECTOR_HELLO_METHOD,
    ChannelCloseParams, ChannelDataParams, ChannelExitParams, ChannelId, ChannelKind,
    ChannelOpenParams, ChannelResizeParams, ChannelSignalParams, ChannelStdinParams, ChannelStream,
    ConnectorHelloParams, EmptyResult, HostInfo, RequestMessage, ResponseMessage, RpcMessage,
    SESSION_ERROR_METHOD, SESSION_SHUTDOWN_METHOD, SessionErrorParams, SignalName,
    WEBSOCKET_SUBPROTOCOL,
};

const CONNECTOR_HELLO_ID: u64 = 1;
pub const DEFAULT_IDLE_TIMEOUT_SECONDS: u64 = 15;
const DEFAULT_SHELL_PATH: &str = "/bin/sh";

#[derive(Clone, Debug)]
pub struct ConnectOptions {
    pub bearer_token: String,
    pub idle_timeout_seconds: u64,
    pub url: String,
}

#[derive(Debug, Error)]
pub enum ConnectorError {
    #[error("{0}")]
    Message(String),
    #[error("Failed to encode a JSON-RPC message: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
}

enum ChannelControl {
    Close,
    Resize { cols: u16, rows: u16 },
    Signal(SignalName),
    Stdin(Vec<u8>),
}

struct ChannelHandle {
    control: mpsc::UnboundedSender<ChannelControl>,
}

enum InternalEvent {
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
}

pub async fn connect(options: ConnectOptions) -> Result<(), ConnectorError> {
    let mut request = options.url.as_str().into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", options.bearer_token))
            .map_err(|error| ConnectorError::Message(error.to_string()))?,
    );
    request.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(WEBSOCKET_SUBPROTOCOL),
    );
    let (socket, response) = connect_async(request).await?;
    let negotiated = response
        .headers()
        .get(SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok());
    if negotiated != Some(WEBSOCKET_SUBPROTOCOL) {
        return Err(ConnectorError::Message(
            "WebSocket subprotocol negotiation failed.".to_string(),
        ));
    }

    let (mut writer, mut reader) = socket.split();
    let hello = RequestMessage::request(
        CONNECTOR_HELLO_ID,
        CONNECTOR_HELLO_METHOD,
        ConnectorHelloParams {
            capabilities: vec!["exec".to_string(), "pty".to_string()],
            host: Some(HostInfo {
                arch: Some(std::env::consts::ARCH.to_string()),
                os: Some(std::env::consts::OS.to_string()),
            }),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    )?;
    send_message(&mut writer, &RpcMessage::Request(hello)).await?;
    wait_for_hello_ack(&mut reader).await?;

    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalEvent>();
    let mut channels = HashMap::<ChannelId, ChannelHandle>::new();
    let idle_timeout = Duration::from_secs(options.idle_timeout_seconds);

    loop {
        tokio::select! {
            event = internal_rx.recv() => {
                let Some(event) = event else {
                    break;
                };
                match event {
                    InternalEvent::Data { channel_id, data, stream } => {
                        send_notification(
                            &mut writer,
                            CHANNEL_DATA_METHOD,
                            ChannelDataParams {
                                channel_id,
                                data: STANDARD.encode(data),
                                stream,
                            },
                        )
                        .await?;
                    }
                    InternalEvent::Error { channel_id, message } => {
                        send_notification(
                            &mut writer,
                            SESSION_ERROR_METHOD,
                            SessionErrorParams { channel_id, message },
                        )
                        .await?;
                    }
                    InternalEvent::Exit { channel_id, code } => {
                        channels.remove(&channel_id);
                        send_notification(
                            &mut writer,
                            CHANNEL_EXIT_METHOD,
                            ChannelExitParams { channel_id, code },
                        )
                        .await?;
                    }
                }
            }
            message = reader.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<RpcMessage>(&text) {
                            Ok(RpcMessage::Request(request)) => {
                                if let Some(id) = request.id {
                                    let response =
                                        handle_request(id, &request, &mut channels, &internal_tx);
                                    send_message(&mut writer, &RpcMessage::Response(response))
                                        .await?;
                                } else if !handle_notification(&request, &channels, &internal_tx) {
                                    break;
                                }
                            }
                            Ok(RpcMessage::Response(_)) => {
                                return Err(ConnectorError::Message(
                                    "Unexpected JSON-RPC response from the server.".to_string(),
                                ));
                            }
                            Err(error) => {
                                send_notification(
                                    &mut writer,
                                    SESSION_ERROR_METHOD,
                                    SessionErrorParams {
                                        channel_id: None,
                                        message: format!("Invalid JSON-RPC message: {error}"),
                                    },
                                )
                                .await?;
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        writer.send(Message::Pong(payload)).await?;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Binary(_))) => {
                        send_notification(
                            &mut writer,
                            SESSION_ERROR_METHOD,
                            SessionErrorParams {
                                channel_id: None,
                                message: "Binary websocket frames are not supported.".to_string(),
                            },
                        )
                        .await?;
                        break;
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(error)) => return Err(error.into()),
                }
            }
            _ = tokio::time::sleep(idle_timeout), if channels.is_empty() => {
                break;
            }
        }
    }

    for handle in channels.into_values() {
        let _ = handle.control.send(ChannelControl::Close);
    }
    let _ = writer.send(Message::Close(None)).await;
    Ok(())
}

fn handle_request(
    id: u64,
    request: &RequestMessage,
    channels: &mut HashMap<ChannelId, ChannelHandle>,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
) -> ResponseMessage {
    match request.method.as_str() {
        CHANNEL_OPEN_METHOD => match request.parse_params::<ChannelOpenParams>() {
            Ok(params) => match start_channel(params, internal_tx.clone()) {
                Ok((channel_id, handle)) => {
                    channels.insert(channel_id, handle);
                    ResponseMessage::success(id, EmptyResult {}).expect("serializable result")
                }
                Err(error) => ResponseMessage::internal_error(id, error),
            },
            Err(error) => ResponseMessage::invalid_params(id, error.to_string()),
        },
        method => ResponseMessage::method_not_found(id, method),
    }
}

fn handle_notification(
    request: &RequestMessage,
    channels: &HashMap<ChannelId, ChannelHandle>,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
) -> bool {
    match request.method.as_str() {
        CHANNEL_STDIN_METHOD => match request.parse_params::<ChannelStdinParams>() {
            Ok(params) => match decode_payload(&params.data) {
                Ok(data) => {
                    send_control(channels, params.channel_id, ChannelControl::Stdin(data));
                }
                Err(error) => {
                    let _ = internal_tx.send(InternalEvent::Error {
                        channel_id: Some(params.channel_id),
                        message: format!("Invalid base64 payload: {error}"),
                    });
                }
            },
            Err(error) => {
                let _ = internal_tx.send(InternalEvent::Error {
                    channel_id: None,
                    message: format!("Invalid channel.stdin params: {error}"),
                });
            }
        },
        CHANNEL_RESIZE_METHOD => match request.parse_params::<ChannelResizeParams>() {
            Ok(params) => send_control(
                channels,
                params.channel_id,
                ChannelControl::Resize {
                    cols: params.cols,
                    rows: params.rows,
                },
            ),
            Err(error) => {
                let _ = internal_tx.send(InternalEvent::Error {
                    channel_id: None,
                    message: format!("Invalid channel.resize params: {error}"),
                });
            }
        },
        CHANNEL_SIGNAL_METHOD => match request.parse_params::<ChannelSignalParams>() {
            Ok(params) => send_control(
                channels,
                params.channel_id,
                ChannelControl::Signal(params.signal),
            ),
            Err(error) => {
                let _ = internal_tx.send(InternalEvent::Error {
                    channel_id: None,
                    message: format!("Invalid channel.signal params: {error}"),
                });
            }
        },
        CHANNEL_CLOSE_METHOD => match request.parse_params::<ChannelCloseParams>() {
            Ok(params) => send_control(channels, params.channel_id, ChannelControl::Close),
            Err(error) => {
                let _ = internal_tx.send(InternalEvent::Error {
                    channel_id: None,
                    message: format!("Invalid channel.close params: {error}"),
                });
            }
        },
        SESSION_SHUTDOWN_METHOD => return false,
        method => {
            let _ = internal_tx.send(InternalEvent::Error {
                channel_id: None,
                message: format!("Unsupported notification '{method}'."),
            });
            return false;
        }
    }

    true
}

fn decode_payload(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    STANDARD.decode(data)
}

fn send_control(
    channels: &HashMap<ChannelId, ChannelHandle>,
    channel_id: ChannelId,
    control: ChannelControl,
) {
    if let Some(handle) = channels.get(&channel_id) {
        let _ = handle.control.send(control);
    }
}

fn start_channel(
    params: ChannelOpenParams,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) -> Result<(ChannelId, ChannelHandle), String> {
    let channel_id = params.channel_id.clone();
    let control = match params.kind {
        ChannelKind::Exec => start_exec_channel(params, internal_tx)?,
        ChannelKind::Pty => start_pty_channel(params, internal_tx)?,
    };
    Ok((channel_id, ChannelHandle { control }))
}

fn start_exec_channel(
    params: ChannelOpenParams,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) -> Result<mpsc::UnboundedSender<ChannelControl>, String> {
    let mut command = Command::new(DEFAULT_SHELL_PATH);
    command.arg("-lc").arg(&params.command);
    if let Some(cwd) = params.cwd.as_deref() {
        command.current_dir(cwd);
    }
    command.envs(params.env);
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start exec channel: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Exec channel stdout was not available.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Exec channel stderr was not available.".to_string())?;
    let stdin = child.stdin.take();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    tokio::spawn(run_exec_channel(
        params.channel_id,
        child,
        stdin,
        stdout,
        stderr,
        control_rx,
        internal_tx,
    ));
    Ok(control_tx)
}

async fn run_exec_channel(
    channel_id: ChannelId,
    mut child: Child,
    mut stdin: Option<ChildStdin>,
    stdout: impl AsyncRead + Send + Unpin + 'static,
    stderr: impl AsyncRead + Send + Unpin + 'static,
    mut control_rx: mpsc::UnboundedReceiver<ChannelControl>,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) {
    tokio::spawn(stream_pipe(
        channel_id.clone(),
        ChannelStream::Stdout,
        stdout,
        internal_tx.clone(),
    ));
    tokio::spawn(stream_pipe(
        channel_id.clone(),
        ChannelStream::Stderr,
        stderr,
        internal_tx.clone(),
    ));

    loop {
        tokio::select! {
            status = child.wait() => {
                let code = status.ok().and_then(|value| value.code());
                let _ = internal_tx.send(InternalEvent::Exit { channel_id: channel_id.clone(), code });
                break;
            }
            control = control_rx.recv() => {
                let Some(control) = control else {
                    let _ = child.start_kill();
                    continue;
                };
                match control {
                    ChannelControl::Close
                    | ChannelControl::Signal(SignalName::Kill | SignalName::Term) => {
                        let _ = child.start_kill();
                    }
                    ChannelControl::Resize { .. } => {}
                    ChannelControl::Stdin(data) => {
                        if let Some(stdin) = stdin.as_mut() {
                            if let Err(error) = stdin.write_all(&data).await {
                                let _ = internal_tx.send(InternalEvent::Error {
                                    channel_id: Some(channel_id.clone()),
                                    message: format!("Failed to write to exec stdin: {error}"),
                                });
                                let _ = child.start_kill();
                                continue;
                            }
                            if let Err(error) = stdin.flush().await {
                                let _ = internal_tx.send(InternalEvent::Error {
                                    channel_id: Some(channel_id.clone()),
                                    message: format!("Failed to flush exec stdin: {error}"),
                                });
                                let _ = child.start_kill();
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn stream_pipe(
    channel_id: ChannelId,
    stream: ChannelStream,
    mut pipe: impl AsyncRead + Unpin,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) {
    let mut buffer = [0_u8; 4096];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let _ = internal_tx.send(InternalEvent::Data {
                    channel_id: channel_id.clone(),
                    data: buffer[..read].to_vec(),
                    stream,
                });
            }
            Err(error) => {
                let _ = internal_tx.send(InternalEvent::Error {
                    channel_id: Some(channel_id.clone()),
                    message: format!("Failed to read channel output: {error}"),
                });
                break;
            }
        }
    }
}

fn start_pty_channel(
    params: ChannelOpenParams,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) -> Result<mpsc::UnboundedSender<ChannelControl>, String> {
    let size = params
        .pty
        .ok_or_else(|| "PTY channels require an initial PTY size.".to_string())?;
    let pair = native_pty_system()
        .openpty(PortablePtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Failed to allocate a PTY: {error}"))?;

    let mut command = CommandBuilder::new(DEFAULT_SHELL_PATH);
    command.arg("-lc");
    command.arg(params.command);
    if let Some(cwd) = params.cwd {
        command.cwd(cwd);
    }
    for (name, value) in params.env {
        command.env(name, value);
    }

    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to start PTY channel: {error}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Failed to clone the PTY reader: {error}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("Failed to acquire the PTY writer: {error}"))?;
    let killer = child.clone_killer();
    let master = pair.master;
    let channel_id = params.channel_id.clone();
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<ChannelControl>();

    let reader_channel_id = channel_id.clone();
    let reader_internal_tx = internal_tx.clone();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let _ = reader_internal_tx.send(InternalEvent::Data {
                        channel_id: reader_channel_id.clone(),
                        data: buffer[..read].to_vec(),
                        stream: ChannelStream::Pty,
                    });
                }
                Err(error) => {
                    let _ = reader_internal_tx.send(InternalEvent::Error {
                        channel_id: Some(reader_channel_id.clone()),
                        message: format!("Failed to read PTY output: {error}"),
                    });
                    break;
                }
            }
        }
    });

    let wait_channel_id = channel_id.clone();
    let wait_internal_tx = internal_tx.clone();
    thread::spawn(move || match child.wait() {
        Ok(status) => {
            let _ = wait_internal_tx.send(InternalEvent::Exit {
                channel_id: wait_channel_id,
                code: Some(status.exit_code() as i32),
            });
        }
        Err(error) => {
            let _ = wait_internal_tx.send(InternalEvent::Error {
                channel_id: Some(wait_channel_id.clone()),
                message: format!("PTY wait failed: {error}"),
            });
            let _ = wait_internal_tx.send(InternalEvent::Exit {
                channel_id: wait_channel_id,
                code: None,
            });
        }
    });

    let control_channel_id = channel_id.clone();
    let control_internal_tx = internal_tx;
    thread::spawn(move || {
        let mut killer = killer;
        while let Some(control) = control_rx.blocking_recv() {
            match control {
                ChannelControl::Close
                | ChannelControl::Signal(SignalName::Kill | SignalName::Term) => {
                    let _ = killer.kill();
                    break;
                }
                ChannelControl::Resize { cols, rows } => {
                    if let Err(error) = master.resize(PortablePtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    }) {
                        let _ = control_internal_tx.send(InternalEvent::Error {
                            channel_id: Some(control_channel_id.clone()),
                            message: format!("Failed to resize the PTY: {error}"),
                        });
                    }
                }
                ChannelControl::Stdin(data) => {
                    if let Err(error) = writer.write_all(&data).and_then(|_| writer.flush()) {
                        let _ = control_internal_tx.send(InternalEvent::Error {
                            channel_id: Some(control_channel_id.clone()),
                            message: format!("Failed to write to the PTY: {error}"),
                        });
                        let _ = killer.kill();
                        break;
                    }
                }
            }
        }
    });
    Ok(control_tx)
}

async fn wait_for_hello_ack(
    reader: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Result<(), ConnectorError> {
    loop {
        let Some(message) = reader.next().await else {
            return Err(ConnectorError::Message(
                "The server closed the connection before connector.hello completed.".to_string(),
            ));
        };
        match message? {
            Message::Text(text) => match serde_json::from_str::<RpcMessage>(&text)? {
                RpcMessage::Response(response) if response.id == CONNECTOR_HELLO_ID => {
                    if let Some(error) = response.error {
                        return Err(ConnectorError::Message(error.message));
                    }
                    return Ok(());
                }
                _ => {
                    return Err(ConnectorError::Message(
                        "The server replied with an unexpected message before connector.hello completed."
                            .to_string(),
                    ));
                }
            },
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Close(_) => {
                return Err(ConnectorError::Message(
                    "The server closed the connection before connector.hello completed."
                        .to_string(),
                ));
            }
            Message::Binary(_) => {
                return Err(ConnectorError::Message(
                    "The server sent a binary frame during connector.hello.".to_string(),
                ));
            }
            Message::Frame(_) => {}
        }
    }
}

async fn send_message(
    writer: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    message: &RpcMessage,
) -> Result<(), ConnectorError> {
    writer
        .send(Message::Text(serde_json::to_string(message)?.into()))
        .await?;
    Ok(())
}

async fn send_notification(
    writer: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    method: &str,
    params: impl serde::Serialize,
) -> Result<(), ConnectorError> {
    send_message(
        writer,
        &RpcMessage::Request(RequestMessage::notification(method, params)?),
    )
    .await
}
