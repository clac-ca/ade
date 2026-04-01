use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use reverse_connect::{
    ConnectOptions, connect,
    protocol::{
        CHANNEL_CLOSE_METHOD, CHANNEL_DATA_METHOD, CHANNEL_EXIT_METHOD, CHANNEL_OPEN_METHOD,
        CHANNEL_STDIN_METHOD, CONNECTOR_HELLO_METHOD, ChannelCloseParams, ChannelDataParams,
        ChannelExitParams, ChannelKind, ChannelOpenParams, ChannelStdinParams, ChannelStream,
        ConnectorHelloParams, EmptyResult, PtySize, RequestMessage, ResponseMessage, RpcMessage,
        SESSION_ERROR_METHOD, SESSION_SHUTDOWN_METHOD,
    },
};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
        http::header,
    },
};

type ServerSocket = WebSocketStream<tokio::net::TcpStream>;

struct AbortOnDrop<T>(Option<tokio::task::JoinHandle<T>>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

async fn start_server(
    token: &str,
) -> (
    String,
    oneshot::Receiver<Result<ServerSocket, String>>,
    AbortOnDrop<Result<(), String>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let token = token.to_string();
    let (socket_tx, socket_rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
        let result = accept_hdr_async(stream, move |request: &Request, response: Response| {
            let auth = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some(
                        "missing authorization header".to_string(),
                    ))
                })?;
            if auth != format!("Bearer {token}") {
                return Err(
                    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some(
                        "invalid authorization header".to_string(),
                    )),
                );
            }
            Ok(response)
        })
        .await;
        match result {
            Ok(socket) => {
                let _ = socket_tx.send(Ok(socket));
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                let _ = socket_tx.send(Err(message.clone()));
                Err(message)
            }
        }
    });
    (
        format!("ws://{address}/"),
        socket_rx,
        AbortOnDrop(Some(handle)),
    )
}

async fn spawn_connector(
    url: String,
    token: String,
) -> (oneshot::Receiver<Result<(), String>>, AbortOnDrop<()>) {
    let (result_tx, result_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let result = connect(ConnectOptions {
            bearer_token: token,
            idle_timeout_seconds: 1,
            url,
        })
        .await
        .map_err(|error| error.to_string());
        let _ = result_tx.send(result);
    });
    (result_rx, AbortOnDrop(Some(task)))
}

struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.start_kill();
        }
    }
}

impl KillOnDrop {
    async fn wait_with_output(mut self) -> std::process::Output {
        self.0
            .take()
            .expect("connector process missing")
            .wait_with_output()
            .await
            .expect("failed to wait for connector process")
    }
}

fn connector_binary() -> &'static str {
    env!("CARGO_BIN_EXE_reverse-connect")
}

fn spawn_connector_cli(url: String, token: String) -> KillOnDrop {
    let child = Command::new(connector_binary())
        .arg("connect")
        .arg("--url")
        .arg(url)
        .arg("--bearer-token")
        .arg(token)
        .arg("--idle-timeout-seconds")
        .arg("1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start reverse-connect");
    KillOnDrop(Some(child))
}

async fn next_message(socket: &mut ServerSocket) -> RpcMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("timed out waiting for websocket message")
        {
            Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
            Some(Ok(Message::Ping(payload))) => {
                socket.send(Message::Pong(payload)).await.unwrap();
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Binary(_))) | Some(Ok(Message::Close(_))) | None => {
                panic!("unexpected websocket close")
            }
            Some(Ok(Message::Frame(_))) => {}
            Some(Err(error)) => panic!("websocket error: {error}"),
        }
    }
}

async fn complete_hello(socket: &mut ServerSocket) {
    let RpcMessage::Request(hello) = next_message(socket).await else {
        panic!("expected connector.hello request");
    };
    assert_eq!(hello.method, CONNECTOR_HELLO_METHOD);
    let params: ConnectorHelloParams = hello.parse_params().unwrap();
    assert!(params.capabilities.iter().any(|value| value == "exec"));
    assert!(params.capabilities.iter().any(|value| value == "pty"));
    let response = ResponseMessage::success(hello.id.unwrap(), EmptyResult {}).unwrap();
    socket
        .send(Message::Text(
            serde_json::to_string(&RpcMessage::Response(response))
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
}

async fn send_request(
    socket: &mut ServerSocket,
    id: u64,
    method: &str,
    params: impl serde::Serialize,
) {
    let request = RequestMessage::request(id, method, params).unwrap();
    socket
        .send(Message::Text(
            serde_json::to_string(&RpcMessage::Request(request))
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
}

async fn send_notification(socket: &mut ServerSocket, method: &str, params: impl serde::Serialize) {
    let notification = RequestMessage::notification(method, params).unwrap();
    socket
        .send(Message::Text(
            serde_json::to_string(&RpcMessage::Request(notification))
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn connector_sends_hello_before_opening_channels() {
    let token = "secret-token";
    let (url, socket_rx, _server_task) = start_server(token).await;
    let (connector_rx, _connector_task) = spawn_connector(url, token.to_string()).await;
    let mut socket = tokio::time::timeout(Duration::from_secs(5), socket_rx)
        .await
        .expect("connector never connected")
        .unwrap()
        .unwrap();

    complete_hello(&mut socket).await;

    send_notification(&mut socket, SESSION_SHUTDOWN_METHOD, serde_json::json!({})).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), connector_rx)
            .await
            .expect("connector did not stop")
            .unwrap(),
        Ok(())
    );
}

#[tokio::test]
async fn exec_channels_stream_stdout_and_exit() {
    let token = "secret-token";
    let (url, socket_rx, _server_task) = start_server(token).await;
    let (connector_rx, _connector_task) = spawn_connector(url, token.to_string()).await;
    let mut socket = tokio::time::timeout(Duration::from_secs(5), socket_rx)
        .await
        .expect("connector never connected")
        .unwrap()
        .unwrap();
    complete_hello(&mut socket).await;

    send_request(
        &mut socket,
        10,
        CHANNEL_OPEN_METHOD,
        ChannelOpenParams {
            channel_id: reverse_connect::protocol::ChannelId::new("exec-1"),
            command: "printf 'hello\\n'".to_string(),
            cwd: None,
            env: Default::default(),
            kind: ChannelKind::Exec,
            pty: None,
        },
    )
    .await;

    let RpcMessage::Response(open_response) = next_message(&mut socket).await else {
        panic!("expected channel.open response");
    };
    assert_eq!(open_response.id, 10);
    assert!(open_response.error.is_none());

    let mut saw_output = false;
    let mut saw_exit = false;
    while !(saw_output && saw_exit) {
        let RpcMessage::Request(message) = next_message(&mut socket).await else {
            panic!("expected connector notification");
        };
        match message.method.as_str() {
            CHANNEL_DATA_METHOD => {
                let params: ChannelDataParams = message.parse_params().unwrap();
                if params.stream == ChannelStream::Stdout {
                    assert_eq!(STANDARD.decode(params.data).unwrap(), b"hello\n");
                    saw_output = true;
                }
            }
            CHANNEL_EXIT_METHOD => {
                let params: ChannelExitParams = message.parse_params().unwrap();
                assert_eq!(params.code, Some(0));
                saw_exit = true;
            }
            other => panic!("unexpected method: {other}"),
        }
    }

    send_notification(&mut socket, SESSION_SHUTDOWN_METHOD, serde_json::json!({})).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), connector_rx)
            .await
            .expect("connector did not stop")
            .unwrap(),
        Ok(())
    );
}

#[tokio::test]
async fn pty_channels_echo_input_and_exit_when_closed() {
    let token = "secret-token";
    let (url, socket_rx, _server_task) = start_server(token).await;
    let (connector_rx, _connector_task) = spawn_connector(url, token.to_string()).await;
    let mut socket = tokio::time::timeout(Duration::from_secs(5), socket_rx)
        .await
        .expect("connector never connected")
        .unwrap()
        .unwrap();
    complete_hello(&mut socket).await;

    send_request(
        &mut socket,
        20,
        CHANNEL_OPEN_METHOD,
        ChannelOpenParams {
            channel_id: reverse_connect::protocol::ChannelId::new("pty-1"),
            command: "cat".to_string(),
            cwd: None,
            env: Default::default(),
            kind: ChannelKind::Pty,
            pty: Some(PtySize { cols: 80, rows: 24 }),
        },
    )
    .await;

    let _ = next_message(&mut socket).await;
    send_notification(
        &mut socket,
        CHANNEL_STDIN_METHOD,
        ChannelStdinParams {
            channel_id: reverse_connect::protocol::ChannelId::new("pty-1"),
            data: STANDARD.encode(b"hello from pty\n"),
        },
    )
    .await;

    let mut saw_output = false;
    while !saw_output {
        let RpcMessage::Request(message) = next_message(&mut socket).await else {
            panic!("expected connector notification");
        };
        if message.method == CHANNEL_DATA_METHOD {
            let params: ChannelDataParams = message.parse_params().unwrap();
            if params.stream == ChannelStream::Pty {
                let text =
                    String::from_utf8_lossy(&STANDARD.decode(params.data).unwrap()).into_owned();
                if text.contains("hello from pty") {
                    saw_output = true;
                }
            }
        }
    }

    send_notification(
        &mut socket,
        CHANNEL_CLOSE_METHOD,
        ChannelCloseParams {
            channel_id: reverse_connect::protocol::ChannelId::new("pty-1"),
        },
    )
    .await;

    loop {
        let RpcMessage::Request(message) = next_message(&mut socket).await else {
            panic!("expected connector notification");
        };
        if message.method == CHANNEL_EXIT_METHOD {
            break;
        }
    }

    send_notification(&mut socket, SESSION_SHUTDOWN_METHOD, serde_json::json!({})).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), connector_rx)
            .await
            .expect("connector did not stop")
            .unwrap(),
        Ok(())
    );
}

#[tokio::test]
async fn invalid_json_reports_a_session_error_and_stops_the_connector() {
    let token = "secret-token";
    let (url, socket_rx, _server_task) = start_server(token).await;
    let (connector_rx, _connector_task) = spawn_connector(url, token.to_string()).await;
    let mut socket = tokio::time::timeout(Duration::from_secs(5), socket_rx)
        .await
        .expect("connector never connected")
        .unwrap()
        .unwrap();
    complete_hello(&mut socket).await;

    socket
        .send(Message::Text("{not-json".to_string().into()))
        .await
        .unwrap();

    let RpcMessage::Request(error) = next_message(&mut socket).await else {
        panic!("expected session.error");
    };
    assert_eq!(error.method, SESSION_ERROR_METHOD);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), connector_rx)
            .await
            .expect("connector did not stop")
            .unwrap(),
        Ok(())
    );
}

#[tokio::test]
async fn cli_connects_over_websocket_and_runs_exec_channels() {
    let token = "secret-token";
    let (url, socket_rx, _server_task) = start_server(token).await;
    let connector = spawn_connector_cli(url, token.to_string());
    let mut socket = tokio::time::timeout(Duration::from_secs(5), socket_rx)
        .await
        .expect("connector never connected")
        .unwrap()
        .unwrap();
    complete_hello(&mut socket).await;

    send_request(
        &mut socket,
        30,
        CHANNEL_OPEN_METHOD,
        ChannelOpenParams {
            channel_id: reverse_connect::protocol::ChannelId::new("exec-cli"),
            command: "printf 'hello\\n'".to_string(),
            cwd: None,
            env: Default::default(),
            kind: ChannelKind::Exec,
            pty: None,
        },
    )
    .await;

    let RpcMessage::Response(open_response) = next_message(&mut socket).await else {
        panic!("expected channel.open response");
    };
    assert_eq!(open_response.id, 30);
    assert!(open_response.error.is_none());

    let mut saw_output = false;
    let mut saw_exit = false;
    while !(saw_output && saw_exit) {
        let RpcMessage::Request(message) = next_message(&mut socket).await else {
            panic!("expected connector notification");
        };
        match message.method.as_str() {
            CHANNEL_DATA_METHOD => {
                let params: ChannelDataParams = message.parse_params().unwrap();
                if params.stream == ChannelStream::Stdout {
                    assert_eq!(STANDARD.decode(params.data).unwrap(), b"hello\n");
                    saw_output = true;
                }
            }
            CHANNEL_EXIT_METHOD => {
                let params: ChannelExitParams = message.parse_params().unwrap();
                assert_eq!(params.code, Some(0));
                saw_exit = true;
            }
            other => panic!("unexpected method: {other}"),
        }
    }

    send_notification(&mut socket, SESSION_SHUTDOWN_METHOD, serde_json::json!({})).await;
    let output = tokio::time::timeout(Duration::from_secs(5), connector.wait_with_output())
        .await
        .expect("connector process did not stop");
    assert!(
        output.status.success(),
        "reverse-connect exited with status {:?}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn cli_reports_connection_errors_to_stderr() {
    let connector = spawn_connector_cli("ws://127.0.0.1:9/".to_string(), "secret-token".to_string());
    let output = tokio::time::timeout(Duration::from_secs(5), connector.wait_with_output())
        .await
        .expect("connector process did not stop");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reverse-connect failed:"));
}
