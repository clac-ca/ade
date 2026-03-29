use std::{
    collections::HashMap,
    path::Path as FsPath,
    sync::{Arc, Mutex},
    time::Duration,
};

use ade_api::{
    readiness::{CreateReadinessControllerOptions, ReadinessController, ReadinessPhase},
    router::{AppState, create_app},
    run_store::InMemoryRunStore,
    runs::RunService,
    session::SessionService,
    terminal::TerminalService,
    unix_time_ms,
};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Multipart, Path, Query, State},
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use reqwest::{
    Client,
    multipart::{Form, Part},
};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tower::util::ServiceExt;

const RUN_SENTINEL_PREFIX: &str = "__ADE_RUN_RESULT__=";
const RUN_EVENT_SENTINEL_PREFIX: &str = "__ADE_RUN_EVENT__=";

#[derive(Default)]
struct PoolStubState {
    downloaded_names: Vec<String>,
    execution_codes: Vec<String>,
    identifiers: Vec<String>,
    uploaded_files: HashMap<String, Vec<u8>>,
    uploaded_names: Vec<String>,
}

#[derive(Clone)]
struct PoolStub {
    options: PoolStubOptions,
    state: Arc<Mutex<PoolStubState>>,
}

#[derive(Clone, Copy)]
struct PoolStubOptions {
    auto_connect_terminal_bridge: bool,
    terminal_bridge_delay_ms: u64,
    terminal_execution_delay_ms: u64,
}

impl Default for PoolStubOptions {
    fn default() -> Self {
        Self {
            auto_connect_terminal_bridge: true,
            terminal_bridge_delay_ms: 0,
            terminal_execution_delay_ms: 0,
        }
    }
}

#[derive(serde::Deserialize)]
struct IdentifierQuery {
    #[serde(rename = "api-version")]
    #[allow(dead_code)]
    api_version: Option<String>,
    identifier: String,
    path: Option<String>,
    #[allow(dead_code)]
    recursive: Option<bool>,
}

fn ready_state() -> ReadinessController {
    let readiness = ReadinessController::new(CreateReadinessControllerOptions {
        database_ok: Some(true),
        last_checked_at: Some(unix_time_ms()),
        phase: Some(ReadinessPhase::Ready),
        ..CreateReadinessControllerOptions::default()
    });
    readiness.mark_ready();
    readiness
}

async fn stub_upload_file(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let field = multipart
        .next_field()
        .await
        .unwrap()
        .expect("multipart file");
    let filename = field.file_name().unwrap().to_string();
    let bytes = field.bytes().await.unwrap();
    let stored_name = match query.path.as_deref() {
        Some(path) if !path.is_empty() && path != "." => format!("{path}/{filename}"),
        _ => filename.clone(),
    };
    let mut state = stub.state.lock().unwrap();
    state.identifiers.push(query.identifier.clone());
    state.uploaded_names.push(stored_name.clone());
    state.uploaded_files.insert(
        format!("{}::{stored_name}", query.identifier),
        bytes.to_vec(),
    );

    Json(json!({
        "directory": stored_name
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .unwrap_or("."),
        "name": stored_name
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(stored_name.as_str()),
        "sizeInBytes": bytes.len(),
        "type": "file",
    }))
}

async fn stub_list_files(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
) -> impl IntoResponse {
    let mut state = stub.state.lock().unwrap();
    state.identifiers.push(query.identifier.clone());
    let prefix = format!("{}::", query.identifier);
    let files = state
        .uploaded_files
        .iter()
        .filter_map(|(key, content)| key.strip_prefix(&prefix).map(|name| (name, content)))
        .map(|(filename, content)| {
            json!({
                "directory": filename
                    .rsplit_once('/')
                    .map(|(directory, _)| directory)
                    .unwrap_or("."),
                "name": filename
                    .rsplit_once('/')
                    .map(|(_, name)| name)
                    .unwrap_or(filename),
                "sizeInBytes": content.len(),
                "type": "file",
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "value": files }))
}

async fn stub_get_file(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
    Path(path): Path<String>,
) -> impl IntoResponse {
    let mut state = stub.state.lock().unwrap();
    state.identifiers.push(query.identifier.clone());
    let filename = path
        .strip_suffix("/content")
        .unwrap_or_else(|| panic!("missing /content suffix: {path}"));
    let stored_name = match query.path.as_deref() {
        Some(path) if !path.is_empty() && path != "." => format!("{path}/{filename}"),
        _ => filename.to_string(),
    };
    state.downloaded_names.push(stored_name.clone());
    let key = format!("{}::{stored_name}", query.identifier);
    let content = state
        .uploaded_files
        .get(&key)
        .cloned()
        .unwrap_or_else(|| panic!("missing file: {stored_name}"));
    (StatusCode::OK, Bytes::from(content)).into_response()
}

async fn stub_execute(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let code = body["code"].as_str().unwrap().to_string();
    let identifier = query.identifier.clone();
    {
        let mut state = stub.state.lock().unwrap();
        state.identifiers.push(identifier.clone());
        state.execution_codes.push(code.clone());
    }

    if code.contains("pty.openpty()")
        && code.contains("websockets.connect")
        && let Some(bridge_url) = extract_bridge_url(&code)
    {
        if stub.options.terminal_bridge_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(stub.options.terminal_bridge_delay_ms)).await;
        }

        if stub.options.auto_connect_terminal_bridge
            && let Ok((mut socket, _response)) = connect_async(&bridge_url).await
        {
            let _ = socket
                .send(Message::Text(r#"{"type":"ready"}"#.into()))
                .await;
            let _ = socket
                .send(Message::Text(
                    r#"{"type":"output","data":"terminal-ok\r\n"}"#.into(),
                ))
                .await;
            let _ = socket
                .send(Message::Text(r#"{"type":"exit","code":0}"#.into()))
                .await;
            let _ = socket.close(None).await;
        }

        if stub.options.terminal_execution_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(
                stub.options.terminal_execution_delay_ms,
            ))
            .await;
        }

        return Json(json!({
            "status": "Succeeded",
            "result": {
                "stdout": "",
                "stderr": "",
                "executionTimeInMilliseconds": 4,
            }
        }));
    }

    if code.contains("websockets.connect")
        && let Some(bridge_url) = extract_bridge_url(&code)
    {
        let config = extract_execution_config(&code).expect("run config");
        let output_dir = config["outputDir"].as_str().expect("output dir");
        let output_path = format!(
            "{}/input.normalized.xlsx",
            output_dir.trim_start_matches("/mnt/data/")
        );
        {
            let mut state = stub.state.lock().unwrap();
            state.uploaded_files.insert(
                format!("{identifier}::{output_path}"),
                b"normalized-output".to_vec(),
            );
        }

        if let Ok((mut socket, _response)) = connect_async(&bridge_url).await {
            let _ = socket
                .send(Message::Text(r#"{"type":"ready"}"#.into()))
                .await;
            let _ = socket
                .send(Message::Text(
                    r#"{"type":"status","phase":"installPackages","state":"started"}"#.into(),
                ))
                .await;
            let _ = socket
                .send(Message::Text(
                    r#"{"type":"status","phase":"installPackages","state":"completed"}"#.into(),
                ))
                .await;
            let _ = socket
                .send(Message::Text(
                    r#"{"type":"status","phase":"executeRun","state":"started"}"#.into(),
                ))
                .await;
            let _ = socket
                .send(Message::Text(
                    format!(
                        r#"{{"type":"result","outputPath":"{output_path}","validationIssues":[]}}"#
                    )
                    .into(),
                ))
                .await;
            let _ = socket.close(None).await;
        }

        return Json(json!({
            "status": "Succeeded",
            "result": {
                "stdout": "",
                "stderr": "",
                "executionTimeInMilliseconds": 4,
            }
        }));
    }

    let stdout = if code.contains(RUN_EVENT_SENTINEL_PREFIX) {
        let config = extract_execution_config(&code).expect("run config");
        let output_dir = config["outputDir"].as_str().expect("output dir");
        let output_path = format!(
            "{}/input.normalized.xlsx",
            output_dir.trim_start_matches("/mnt/data/")
        );
        {
            let mut state = stub.state.lock().unwrap();
            state.uploaded_files.insert(
                format!("{identifier}::{output_path}"),
                b"normalized-output".to_vec(),
            );
        }
        format!(
            "{RUN_EVENT_SENTINEL_PREFIX}{{\"type\":\"status\",\"phase\":\"installPackages\",\"state\":\"started\"}}\n{RUN_EVENT_SENTINEL_PREFIX}{{\"type\":\"status\",\"phase\":\"installPackages\",\"state\":\"completed\"}}\n{RUN_EVENT_SENTINEL_PREFIX}{{\"type\":\"status\",\"phase\":\"executeRun\",\"state\":\"started\"}}\n{RUN_EVENT_SENTINEL_PREFIX}{{\"type\":\"result\",\"outputPath\":\"{output_path}\",\"validationIssues\":[]}}\n"
        )
    } else if code.contains(RUN_SENTINEL_PREFIX) {
        format!(
            "run log\n{RUN_SENTINEL_PREFIX}{{\"outputPath\":\"runs/run-1/output/input.normalized.xlsx\",\"validationIssues\":[]}}\n"
        )
    } else if code.contains("subprocess.run") {
        "pwd\n__ADE_COMMAND_META__={\"exitCode\":7}\n".to_string()
    } else {
        "ok\n".to_string()
    };

    Json(json!({
        "status": "Succeeded",
        "result": {
            "stdout": stdout,
            "stderr": "",
            "executionTimeInMilliseconds": 4,
        }
    }))
}

async fn start_stub_server() -> (
    String,
    Arc<Mutex<PoolStubState>>,
    tokio::task::JoinHandle<()>,
) {
    start_stub_server_with_options(PoolStubOptions::default()).await
}

async fn start_stub_server_with_options(
    options: PoolStubOptions,
) -> (
    String,
    Arc<Mutex<PoolStubState>>,
    tokio::task::JoinHandle<()>,
) {
    let state = Arc::new(Mutex::new(PoolStubState::default()));
    let stub = PoolStub {
        options,
        state: state.clone(),
    };
    let app = Router::new()
        .route("/files", post(stub_upload_file).get(stub_list_files))
        .route("/files/{*path}", get(stub_get_file))
        .route("/executions", post(stub_execute))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{address}"), state, handle)
}

fn app_with_session(
    endpoint: &str,
    config_targets: &[(&str, &str, &FsPath)],
    engine_wheel_path: &FsPath,
) -> axum::Router {
    app_with_session_and_url(
        endpoint,
        "http://127.0.0.1:8000",
        config_targets,
        engine_wheel_path,
    )
}

fn app_with_session_and_url(
    endpoint: &str,
    app_url: &str,
    config_targets: &[(&str, &str, &FsPath)],
    engine_wheel_path: &FsPath,
) -> axum::Router {
    app_with_session_and_url_and_terminal_service(
        endpoint,
        app_url,
        config_targets,
        engine_wheel_path,
    )
    .0
}

fn app_with_session_and_url_and_terminal_service(
    endpoint: &str,
    app_url: &str,
    config_targets: &[(&str, &str, &FsPath)],
    engine_wheel_path: &FsPath,
) -> (axum::Router, Arc<TerminalService>) {
    let config_targets = config_targets
        .iter()
        .map(|(workspace_id, config_version_id, wheel_path)| {
            json!({
                "workspaceId": workspace_id,
                "configVersionId": config_version_id,
                "wheelPath": wheel_path.display().to_string(),
            })
        })
        .collect::<Vec<_>>();
    let env = [
        (
            "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT".to_string(),
            endpoint.to_string(),
        ),
        (
            "ADE_CONFIG_TARGETS".to_string(),
            serde_json::to_string(&config_targets).unwrap(),
        ),
        (
            "ADE_ENGINE_WHEEL_PATH".to_string(),
            engine_wheel_path.display().to_string(),
        ),
        (
            "ADE_SESSION_SECRET".to_string(),
            "test-session-secret".to_string(),
        ),
        ("ADE_APP_URL".to_string(), app_url.to_string()),
    ]
    .into_iter()
    .collect();

    let session_service = Arc::new(SessionService::from_env(&env).unwrap());
    let run_service = Arc::new(
        RunService::from_env(
            &[
                ("ADE_APP_URL".to_string(), app_url.to_string()),
                (
                    "ADE_ARTIFACTS_ROOT".to_string(),
                    engine_wheel_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join("artifacts")
                        .display()
                        .to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            Arc::clone(&session_service),
            Arc::new(InMemoryRunStore::default()),
        )
        .unwrap(),
    );
    let terminal_service =
        Arc::new(TerminalService::from_env(&env, Arc::clone(&session_service)).unwrap());

    let app = create_app(AppState {
        readiness: ready_state(),
        run_service,
        terminal_service: Arc::clone(&terminal_service),
        session_service,
        web_root: None,
    });

    (app, terminal_service)
}

fn extract_execution_config(code: &str) -> Option<Value> {
    let config_line = code
        .lines()
        .find(|line| line.trim_start().starts_with("CONFIG = json.loads("))?;
    let encoded_json = config_line
        .trim()
        .strip_prefix("CONFIG = json.loads(")?
        .strip_suffix(")")?;
    let config_json = serde_json::from_str::<String>(encoded_json).ok()?;
    serde_json::from_str::<Value>(&config_json).ok()
}

fn extract_bridge_url(code: &str) -> Option<String> {
    let config = extract_execution_config(code)?;
    config["bridgeUrl"].as_str().map(str::to_string)
}

fn create_wheels() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let tempdir = tempdir().unwrap();
    let config_v1 = tempdir.path().join("ade_config-0.1.0-py3-none-any.whl");
    let config_v2 = tempdir.path().join("ade_config-0.2.0-py3-none-any.whl");
    let engine = tempdir.path().join("ade_engine-0.1.0-py3-none-any.whl");
    std::fs::write(&config_v1, b"config-wheel-v1").unwrap();
    std::fs::write(&config_v2, b"config-wheel-v2").unwrap();
    std::fs::write(&engine, b"engine-wheel").unwrap();
    (tempdir, config_v1, config_v2, engine)
}

#[tokio::test]
async fn session_routes_proxy_flat_files_and_preserve_scope_isolation() {
    let (endpoint, state, handle) = start_stub_server().await;
    let (_tempdir, config_v1, config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[
            ("workspace-a", "config-v1", config_v1.as_path()),
            ("workspace-b", "config-v2", config_v2.as_path()),
        ],
        &engine,
    );

    let boundary = "test-boundary";
    let payload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n--{boundary}--\r\n"
    );

    let upload = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-a/configs/config-v1/files")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::OK);
    let uploaded =
        serde_json::from_slice::<Value>(&to_bytes(upload.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(uploaded["filename"], "notes.txt");
    assert_eq!(uploaded["size"], 5);

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/workspaces/workspace-a/configs/config-v1/files")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let listed =
        serde_json::from_slice::<Value>(&to_bytes(list.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(listed, json!([{ "filename": "notes.txt", "size": 5 }]));

    let isolated_list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/workspaces/workspace-b/configs/config-v2/files")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let isolated_files = serde_json::from_slice::<Value>(
        &to_bytes(isolated_list.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(isolated_files, json!([]));

    let download = app
        .oneshot(
            Request::builder()
                .uri("/api/workspaces/workspace-a/configs/config-v1/files/notes.txt/content")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(download.into_body(), usize::MAX).await.unwrap(),
        Bytes::from_static(b"hello")
    );

    let identifiers = state.lock().unwrap().identifiers.clone();
    assert!(
        identifiers
            .iter()
            .all(|identifier| identifier.starts_with("cfg-")),
        "session identifiers should be derived server-side",
    );

    handle.abort();
}

#[tokio::test]
async fn removed_session_endpoints_return_not_found() {
    let (endpoint, _state, handle) = start_stub_server().await;
    let (_tempdir, config_v1, config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[
            ("workspace-a", "config-v1", config_v1.as_path()),
            ("workspace-b", "config-v2", config_v2.as_path()),
        ],
        &engine,
    );

    let execution_lookup = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/workspaces/workspace-a/configs/config-v1/executions/abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(execution_lookup.status(), StatusCode::NOT_FOUND);

    let file_metadata = app
        .oneshot(
            Request::builder()
                .uri("/api/workspaces/workspace-a/configs/config-v1/files/notes.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(file_metadata.status(), StatusCode::NOT_FOUND);

    handle.abort();
}

#[tokio::test]
async fn shell_command_requests_return_flat_results_and_session_ids_stay_private() {
    let (endpoint, state, handle) = start_stub_server().await;
    let (_tempdir, config_v1, config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[
            ("workspace-a", "config-v1", config_v1.as_path()),
            ("workspace-b", "config-v2", config_v2.as_path()),
        ],
        &engine,
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-a/configs/config-v1/executions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "shellCommand": "pwd",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let payload =
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(
        payload,
        json!({
            "stdout": "pwd",
            "stderr": "",
            "exitCode": 7,
            "durationMs": 4,
        })
    );

    let execution_codes = state.lock().unwrap().execution_codes.clone();
    assert_eq!(execution_codes.len(), 1);
    assert!(execution_codes[0].contains("subprocess.run"));

    handle.abort();
}

#[tokio::test]
async fn terminal_route_starts_bootstrap_code_and_streams_bridge_events() {
    let (endpoint, state, stub_handle) = start_stub_server().await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = app_with_session_and_url(
        &endpoint,
        &format!("http://{address}"),
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );
    let app_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let terminal_url =
        format!("ws://{address}/api/workspaces/workspace-a/configs/config-v1/terminal");
    let (mut socket, _response) = connect_async(&terminal_url).await.unwrap();

    let ready = socket.next().await.unwrap().unwrap();
    let output = socket.next().await.unwrap().unwrap();
    let maybe_terminal_end = tokio::time::timeout(Duration::from_millis(250), socket.next())
        .await
        .ok()
        .flatten();

    assert_eq!(ready.into_text().unwrap(), r#"{"type":"ready"}"#);
    assert_eq!(
        output.into_text().unwrap(),
        r#"{"type":"output","data":"terminal-ok\r\n"}"#
    );
    if let Some(Ok(Message::Text(exit))) = maybe_terminal_end {
        assert_eq!(exit, r#"{"type":"exit","code":0}"#);
    }

    let execution_codes = state.lock().unwrap().execution_codes.clone();
    assert_eq!(execution_codes.len(), 1);
    assert!(execution_codes[0].contains("pty.openpty()"));
    assert!(execution_codes[0].contains("websockets.connect"));
    assert!(!execution_codes[0].contains("capture_output=True"));

    let _ = socket.close(None).await;
    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn internal_bridge_route_can_only_attach_once() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_bridge: false,
        terminal_bridge_delay_ms: 0,
        terminal_execution_delay_ms: 300,
    })
    .await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = app_with_session_and_url(
        &endpoint,
        &format!("http://{address}"),
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );
    let app_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let terminal_url =
        format!("ws://{address}/api/workspaces/workspace-a/configs/config-v1/terminal");
    let (mut browser_socket, _response) = connect_async(&terminal_url).await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let bridge_url = {
        let execution_codes = state.lock().unwrap().execution_codes.clone();
        extract_bridge_url(&execution_codes[0]).unwrap()
    };

    let (mut bridge_socket, _response) = connect_async(&bridge_url).await.unwrap();
    bridge_socket
        .send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .unwrap();
    bridge_socket
        .send(Message::Text(r#"{"type":"exit","code":0}"#.into()))
        .await
        .unwrap();

    let ready = browser_socket.next().await.unwrap().unwrap();
    let exit = browser_socket.next().await.unwrap().unwrap();
    assert_eq!(ready.into_text().unwrap(), r#"{"type":"ready"}"#);
    assert_eq!(exit.into_text().unwrap(), r#"{"type":"exit","code":0}"#);

    assert!(connect_async(&bridge_url).await.is_err());

    let _ = bridge_socket.close(None).await;
    let _ = browser_socket.close(None).await;
    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn browser_disconnect_before_bridge_ready_clears_pending_state() {
    let (endpoint, _state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_bridge: false,
        terminal_bridge_delay_ms: 0,
        terminal_execution_delay_ms: 300,
    })
    .await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (app, terminal_service) = app_with_session_and_url_and_terminal_service(
        &endpoint,
        &format!("http://{address}"),
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );
    let app_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let terminal_url =
        format!("ws://{address}/api/workspaces/workspace-a/configs/config-v1/terminal");
    let (mut browser_socket, _response) = connect_async(&terminal_url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(terminal_service.pending_count(), 1);

    browser_socket.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(terminal_service.pending_count(), 0);

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn reconnect_while_previous_terminal_is_shutting_down_returns_clear_error() {
    let (endpoint, _state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_bridge: false,
        terminal_bridge_delay_ms: 0,
        terminal_execution_delay_ms: 300,
    })
    .await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = app_with_session_and_url(
        &endpoint,
        &format!("http://{address}"),
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );
    let app_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let terminal_url =
        format!("ws://{address}/api/workspaces/workspace-a/configs/config-v1/terminal");
    let (mut first_socket, _response) = connect_async(&terminal_url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    first_socket.close(None).await.unwrap();

    let (mut second_socket, _response) = connect_async(&terminal_url).await.unwrap();
    let message = second_socket
        .next()
        .await
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert_eq!(
        message,
        r#"{"type":"error","message":"A terminal session for this workspace is still shutting down. Retry in a few seconds."}"#
    );

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn unsupported_execution_fields_are_rejected() {
    let (endpoint, _state, handle) = start_stub_server().await;
    let (_tempdir, config_v1, config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[
            ("workspace-a", "config-v1", config_v1.as_path()),
            ("workspace-b", "config-v2", config_v2.as_path()),
        ],
        &engine,
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-a/configs/config-v1/executions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "stdin": "hello",
                        "shellCommand": "pwd",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        serde_json::from_slice::<Value>(&body).unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );

    handle.abort();
}

#[tokio::test]
async fn raw_python_execution_requests_are_rejected() {
    let (endpoint, _state, handle) = start_stub_server().await;
    let (_tempdir, config_v1, config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[
            ("workspace-a", "config-v1", config_v1.as_path()),
            ("workspace-b", "config-v2", config_v2.as_path()),
        ],
        &engine,
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-a/configs/config-v1/executions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "code": "print('hello')",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        serde_json::from_slice::<Value>(&body).unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );

    handle.abort();
}

#[tokio::test]
async fn run_route_uses_scoped_config_artifacts_and_existing_session_files() {
    let (endpoint, state, handle) = start_stub_server().await;
    let (_tempdir, config_v1, config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[
            ("workspace-a", "config-v1", config_v1.as_path()),
            ("workspace-b", "config-v2", config_v2.as_path()),
        ],
        &engine,
    );

    let upload_boundary = "run-upload-boundary";
    let upload_body = format!(
        "--{upload_boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input.csv\"\r\nContent-Type: text/csv\r\n\r\nname,email\nalice,alice@example.com\n\r\n--{upload_boundary}--\r\n"
    );
    let upload = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-a/configs/config-v1/files")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={upload_boundary}"),
                )
                .body(Body::from(upload_body))
                .unwrap(),
        )
        .await
        .unwrap();
    let uploaded =
        serde_json::from_slice::<Value>(&to_bytes(upload.into_body(), usize::MAX).await.unwrap())
            .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-a/configs/config-v1/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "inputPath": uploaded["filename"],
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let payload =
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    let run_id = payload["runId"].as_str().expect("run id");
    let output_path = payload["outputPath"].as_str().expect("output path");
    assert_eq!(payload["validationIssues"], json!([]));
    assert!(output_path.starts_with(&format!("runs/{run_id}/output/")));
    assert!(output_path.ends_with("input.normalized.xlsx"));
    assert!(payload.get("resultPath").is_none());

    {
        let state = state.lock().unwrap();
        assert_eq!(state.downloaded_names, vec![output_path.to_string()]);
        assert!(state.uploaded_names.iter().any(|name| name == "input.csv"));
        assert!(
            state
                .uploaded_names
                .iter()
                .any(|name| name == "ade_engine-0.1.0-py3-none-any.whl")
        );
        assert!(
            state
                .uploaded_names
                .iter()
                .any(|name| name == "ade_config-0.1.0-py3-none-any.whl")
        );
        assert!(
            state
                .uploaded_names
                .iter()
                .all(|name| !name.starts_with("runs/"))
        );
    }

    let second_upload_boundary = "run-upload-boundary-b";
    let second_upload_body = format!(
        "--{second_upload_boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input.csv\"\r\nContent-Type: text/csv\r\n\r\nname,email\nbob,bob@example.com\n\r\n--{second_upload_boundary}--\r\n"
    );
    let second_upload = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-b/configs/config-v2/files")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={second_upload_boundary}"),
                )
                .body(Body::from(second_upload_body))
                .unwrap(),
        )
        .await
        .unwrap();
    let second_uploaded = serde_json::from_slice::<Value>(
        &to_bytes(second_upload.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let second_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-b/configs/config-v2/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "inputPath": second_uploaded["filename"],
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_response.status(), StatusCode::OK);

    {
        let state = state.lock().unwrap();
        assert!(
            state
                .uploaded_names
                .iter()
                .any(|name| name == "ade_config-0.2.0-py3-none-any.whl")
        );
    }

    handle.abort();
}

#[tokio::test]
async fn async_runs_return_accepted_and_replay_events() {
    let (endpoint, _state, stub_handle) = start_stub_server().await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = app_with_session_and_url(
        &endpoint,
        &format!("http://{address}"),
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );
    let app_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = Client::new();
    let upload = client
        .post(format!(
            "http://{address}/api/workspaces/workspace-a/configs/config-v1/files"
        ))
        .multipart(
            Form::new().part(
                "file",
                Part::bytes(b"name,email\nalice,alice@example.com\n".to_vec())
                    .file_name("input.csv")
                    .mime_str("text/csv")
                    .unwrap(),
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(upload.status(), reqwest::StatusCode::OK);
    let uploaded = upload.json::<Value>().await.unwrap();

    let response = client
        .post(format!(
            "http://{address}/api/workspaces/workspace-a/configs/config-v1/runs"
        ))
        .header("Prefer", "respond-async")
        .json(&json!({
            "inputPath": uploaded["filename"],
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(
        response
            .headers()
            .get("preference-applied")
            .and_then(|value| value.to_str().ok()),
        Some("respond-async")
    );
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .to_string();
    let payload = response.json::<Value>().await.unwrap();
    let run_id = payload["runId"].as_str().expect("run id");
    assert!(location.ends_with(&format!("/runs/{run_id}")));
    assert_eq!(
        payload["eventsUrl"],
        Value::String(format!(
            "/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/events"
        ))
    );
    assert_eq!(payload["status"], "pending");

    let detail_url = format!("http://{address}{location}");
    let mut detail = Value::Null;
    for _ in 0..20 {
        detail = client
            .get(&detail_url)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        if detail["status"] == "succeeded" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(detail["status"], "succeeded");

    let events_url =
        format!("ws://{address}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/events");
    let (mut socket, _response) = connect_async(events_url).await.unwrap();
    socket
        .send(Message::Text(
            r#"{"type":"attach","lastSeenSeq":null}"#.into(),
        ))
        .await
        .unwrap();

    let mut messages = Vec::new();
    while let Some(message) = tokio::time::timeout(Duration::from_millis(250), socket.next())
        .await
        .ok()
        .flatten()
    {
        let message = message.unwrap();
        if let Message::Text(text) = message {
            messages.push(text.to_string());
        }
    }

    assert!(
        messages
            .iter()
            .any(|message| message.contains(r#""type":"hello""#))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains(r#""type":"result""#))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains(r#""type":"complete""#))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains(r#""phase":"uploadArtifacts""#))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains(r#""phase":"persistOutputs""#))
    );

    let _ = socket.close(None).await;
    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn multipart_run_uploads_are_rejected() {
    let (endpoint, _state, handle) = start_stub_server().await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );

    let boundary = "run-boundary";
    let multipart_body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input.csv\"\r\nContent-Type: text/csv\r\n\r\nname,email\nalice,alice@example.com\n\r\n--{boundary}--\r\n"
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-a/configs/config-v1/runs")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    handle.abort();
}

#[tokio::test]
async fn run_route_returns_not_found_for_unknown_config_targets() {
    let (endpoint, _state, handle) = start_stub_server().await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-b/configs/config-v2/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "inputPath": "input.csv",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["message"],
        "Config version 'config-v2' for workspace 'workspace-b' is not configured."
    );

    handle.abort();
}
