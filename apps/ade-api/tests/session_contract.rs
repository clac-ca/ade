use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use ade_api::{
    api::{AppState, create_app},
    readiness::{DatabaseReadiness, ReadinessController, ReadinessPhase, ReadinessSnapshot},
    runs::{InMemoryRunStore, RunService},
    session::SessionService,
    terminal::TerminalService,
    unix_time_ms,
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Multipart, Query, State},
    http::{HeaderName, HeaderValue, Request, StatusCode},
    response::IntoResponse,
    routing::post,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tower::util::ServiceExt;

#[derive(Default)]
struct PoolStubState {
    artifact_download_urls: Vec<String>,
    artifact_upload_urls: Vec<String>,
    execution_codes: Vec<String>,
    identifiers: Vec<String>,
    run_execution_count: usize,
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
    auto_connect_run_bridge: bool,
    auto_connect_terminal_bridge: bool,
    run_bridge_delay_ms: u64,
    run_bridge_disconnect_before_ready_attempts: usize,
    run_execution_delay_ms: u64,
    terminal_bridge_delay_ms: u64,
    terminal_execution_delay_ms: u64,
}

impl Default for PoolStubOptions {
    fn default() -> Self {
        Self {
            auto_connect_run_bridge: true,
            auto_connect_terminal_bridge: true,
            run_bridge_delay_ms: 0,
            run_bridge_disconnect_before_ready_attempts: 0,
            run_execution_delay_ms: 0,
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
}

#[derive(Debug, Eq, PartialEq)]
struct SseEvent {
    data: String,
    event: String,
    id: i64,
}

fn ready_state() -> ReadinessController {
    ReadinessController::new(ReadinessSnapshot {
        database: DatabaseReadiness {
            ok: true,
            last_checked_at: Some(unix_time_ms()),
            ..DatabaseReadiness::default()
        },
        phase: ReadinessPhase::Ready,
    })
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

async fn stub_execute(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let code = body["code"].as_str().unwrap().to_string();
    let identifier = query.identifier.clone();
    {
        let mut state = stub.state.lock().unwrap();
        state.identifiers.push(identifier);
        state.execution_codes.push(code.clone());
    }

    if code.contains("pty.openpty()")
        && code.contains("websockets.sync.client")
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

    if code.contains("websockets.sync.client")
        && let Some(bridge_url) = extract_bridge_url(&code)
    {
        let run_execution_count = {
            let mut state = stub.state.lock().unwrap();
            state.run_execution_count += 1;
            state.run_execution_count
        };
        let config = extract_execution_config(&code).expect("run config");

        if stub.options.run_bridge_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(stub.options.run_bridge_delay_ms)).await;
        }

        if run_execution_count <= stub.options.run_bridge_disconnect_before_ready_attempts {
            if let Ok((mut socket, _response)) = connect_async(&bridge_url).await {
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

        if stub.options.auto_connect_run_bridge {
            let client = Client::new();
            let input_download = config["inputDownload"].as_object().expect("input download");
            let output_upload = config["outputUpload"].as_object().expect("output upload");
            let output_path = config["outputPath"].as_str().expect("output path");

            let _input = request_artifact(&client, input_download, None).await;
            {
                let mut state = stub.state.lock().unwrap();
                state
                    .artifact_download_urls
                    .push(input_download["url"].as_str().unwrap().to_string());
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
                        r#"{"type":"log","phase":"executeRun","level":"info","message":"Loaded 12 rows"}"#.into(),
                    ))
                    .await;
                let _ = socket
                    .send(Message::Text(
                        r#"{"type":"status","phase":"persistOutputs","state":"started"}"#.into(),
                    ))
                    .await;

                request_artifact(&client, output_upload, Some(b"normalized-output".to_vec())).await;
                {
                    let mut state = stub.state.lock().unwrap();
                    state
                        .artifact_upload_urls
                        .push(output_upload["url"].as_str().unwrap().to_string());
                }

                let _ = socket
                    .send(Message::Text(
                        format!(
                            r#"{{"type":"result","outputPath":"{output_path}","validationIssues":[]}}"#
                        )
                        .into(),
                    ))
                    .await;
                let _ = socket
                    .send(Message::Text(
                        r#"{"type":"status","phase":"persistOutputs","state":"completed"}"#.into(),
                    ))
                    .await;
                let _ = socket.close(None).await;
            }
        }

        if stub.options.run_execution_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(stub.options.run_execution_delay_ms)).await;
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

    Json(json!({
        "status": "Succeeded",
        "result": {
            "stdout": "ok\n",
            "stderr": "",
            "executionTimeInMilliseconds": 4,
        }
    }))
}

async fn request_artifact(
    client: &Client,
    access: &serde_json::Map<String, Value>,
    body: Option<Vec<u8>>,
) {
    let method = Method::from_bytes(access["method"].as_str().unwrap().as_bytes()).unwrap();
    let mut request = client.request(method, access["url"].as_str().unwrap());
    for (name, value) in access["headers"].as_object().expect("access headers") {
        request = request.header(name, value.as_str().unwrap());
    }
    if let Some(body) = body {
        request = request.body(body);
    }

    let response = request.send().await.unwrap();
    assert!(
        response.status().is_success(),
        "artifact request failed with status {}",
        response.status()
    );
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
        state: Arc::clone(&state),
    };
    let app = Router::new()
        .route("/files", post(stub_upload_file))
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

    create_app(AppState {
        readiness: ready_state(),
        run_service,
        terminal_service,
        web_root: None,
    })
}

fn artifact_root(engine_wheel_path: &FsPath) -> PathBuf {
    engine_wheel_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("artifacts")
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

fn parse_sse_events(body: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    let mut event_name: Option<String> = None;
    let mut event_id: Option<i64> = None;
    let mut data_lines = Vec::new();

    for line in body.lines() {
        if line.is_empty() {
            if let (Some(event), Some(id)) = (event_name.take(), event_id.take()) {
                events.push(SseEvent {
                    data: data_lines.join("\n"),
                    event,
                    id,
                });
            }
            data_lines.clear();
            continue;
        }

        if line.starts_with(':') {
            continue;
        }

        if let Some(value) = line.strip_prefix("event: ") {
            event_name = Some(value.to_string());
            continue;
        }

        if let Some(value) = line.strip_prefix("id: ") {
            event_id = Some(value.parse().unwrap());
            continue;
        }

        if let Some(value) = line.strip_prefix("data: ") {
            data_lines.push(value.to_string());
        }
    }

    if let (Some(event), Some(id)) = (event_name.take(), event_id.take()) {
        events.push(SseEvent {
            data: data_lines.join("\n"),
            event,
            id,
        });
    }

    events
}

fn resolve_url(base_url: &str, url: &str) -> String {
    if Url::parse(url).is_ok() {
        return url.to_string();
    }

    Url::parse(base_url).unwrap().join(url).unwrap().to_string()
}

async fn upload_input(
    client: &Client,
    base_url: &str,
    filename: &str,
    content_type: &str,
    body: Vec<u8>,
) -> Value {
    let response = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/uploads"
        ))
        .json(&json!({
            "filename": filename,
            "contentType": content_type,
            "size": body.len(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let payload = response.json::<Value>().await.unwrap();

    let upload = payload["upload"].as_object().unwrap();
    let method = Method::from_bytes(upload["method"].as_str().unwrap().as_bytes()).unwrap();
    let mut request = client.request(
        method,
        resolve_url(base_url, upload["url"].as_str().unwrap()),
    );
    for (name, value) in upload["headers"].as_object().unwrap() {
        let name = HeaderName::from_bytes(name.as_bytes()).unwrap();
        let value = HeaderValue::from_str(value.as_str().unwrap()).unwrap();
        request = request.header(name, value);
    }
    let upload_response = request.body(body).send().await.unwrap();
    assert_eq!(upload_response.status(), reqwest::StatusCode::CREATED);

    payload
}

async fn start_run(client: &Client, base_url: &str, input_path: &str) -> reqwest::Response {
    client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs"
        ))
        .json(&json!({
            "inputPath": input_path,
        }))
        .send()
        .await
        .unwrap()
}

async fn wait_for_terminal_status(client: &Client, url: &str, expected: &str) -> Value {
    let mut detail = Value::Null;
    for _ in 0..40 {
        detail = client
            .get(url)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        if detail["status"] == expected {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    panic!("run never reached status {expected}: {detail}");
}

async fn wait_for_output_path(client: &Client, url: &str) -> Value {
    let mut detail = Value::Null;
    for _ in 0..40 {
        detail = client
            .get(url)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        if detail["outputPath"].is_string() {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    panic!("run never exposed outputPath: {detail}");
}

#[tokio::test]
async fn uploads_route_returns_server_chosen_paths_and_direct_upload_instructions() {
    let (endpoint, _state, stub_handle) = start_stub_server().await;
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
                .uri("/api/workspaces/workspace-a/configs/config-v1/uploads")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "filename": "../Quarterly Input.xlsx",
                        "contentType": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                        "size": 1048576,
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
    let file_path = payload["filePath"].as_str().unwrap();
    assert!(file_path.starts_with("workspaces/workspace-a/configs/config-v1/uploads/upl_"));
    assert!(file_path.ends_with("/Quarterly Input.xlsx"));
    assert_eq!(payload["upload"]["method"], "PUT");
    assert_eq!(
        payload["upload"]["headers"]["x-ade-artifact-token"].as_str(),
        Some(
            payload["upload"]["headers"]["x-ade-artifact-token"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(
        payload["upload"]["headers"]["content-type"],
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );
    assert!(
        payload["upload"]["url"]
            .as_str()
            .unwrap()
            .contains("/api/internal/artifacts/workspaces/workspace-a/configs/config-v1/uploads/")
    );
    assert!(
        payload["upload"]["expiresAt"]
            .as_str()
            .unwrap()
            .contains('T')
    );

    stub_handle.abort();
}

#[tokio::test]
async fn create_run_returns_accepted_metadata_and_persists_output_via_artifact_store() {
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

    let client = Client::new();
    let base_url = format!("http://{address}");
    let upload = upload_input(
        &client,
        &base_url,
        "input.csv",
        "text/csv",
        b"name,email\nalice,alice@example.com\n".to_vec(),
    )
    .await;

    let response = start_run(&client, &base_url, upload["filePath"].as_str().unwrap()).await;
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .to_string();
    let payload = response.json::<Value>().await.unwrap();
    let run_id = payload["runId"].as_str().unwrap();
    assert_eq!(payload["status"], "pending");
    assert_eq!(payload["inputPath"], upload["filePath"]);
    assert_eq!(payload["outputPath"], Value::Null);
    assert_eq!(
        payload["eventsUrl"],
        Value::String(format!(
            "/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/events"
        ))
    );
    assert!(location.ends_with(&format!("/runs/{run_id}")));

    let detail =
        wait_for_terminal_status(&client, &format!("{base_url}{location}"), "succeeded").await;
    let output_path = detail["outputPath"].as_str().unwrap();
    assert_eq!(
        output_path,
        format!("workspaces/workspace-a/configs/config-v1/runs/{run_id}/output/normalized.xlsx")
    );
    assert_eq!(detail["validationIssues"], json!([]));

    let output_bytes = std::fs::read(artifact_root(&engine).join(output_path)).unwrap();
    assert_eq!(output_bytes, b"normalized-output");

    let state = state.lock().unwrap();
    assert_eq!(state.artifact_download_urls.len(), 1);
    assert_eq!(state.artifact_upload_urls.len(), 1);
    assert!(
        state
            .uploaded_names
            .iter()
            .all(|name| !name.starts_with("workspaces/")),
        "session uploads should contain only runtime wheels",
    );
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

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn run_events_stream_over_sse_and_resume_from_last_event_id() {
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
    let base_url = format!("http://{address}");
    let upload = upload_input(
        &client,
        &base_url,
        "input.csv",
        "text/csv",
        b"name,email\nalice,alice@example.com\n".to_vec(),
    )
    .await;
    let created = start_run(&client, &base_url, upload["filePath"].as_str().unwrap())
        .await
        .json::<Value>()
        .await
        .unwrap();
    let run_id = created["runId"].as_str().unwrap();
    let detail_url =
        format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}");
    wait_for_terminal_status(&client, &detail_url, "succeeded").await;

    let events_text = client
        .get(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/events"
        ))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let events = parse_sse_events(&events_text);
    assert!(!events.is_empty());
    assert_eq!(events.first().unwrap().event, "run.created");
    assert_eq!(events.last().unwrap().event, "run.completed");
    assert!(events.windows(2).all(|pair| pair[0].id < pair[1].id));
    assert!(events.iter().any(|event| event.event == "run.log"
        && serde_json::from_str::<Value>(&event.data).unwrap()["message"] == "Loaded 12 rows"));
    assert!(events.iter().any(|event| event.event == "run.result"
        && serde_json::from_str::<Value>(&event.data).unwrap()["outputPath"]
            == format!(
                "workspaces/workspace-a/configs/config-v1/runs/{run_id}/output/normalized.xlsx"
            )));

    let resume_from = events[2].id;
    let resumed_text = client
        .get(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/events"
        ))
        .header("Accept", "text/event-stream")
        .header("Last-Event-ID", resume_from.to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let resumed = parse_sse_events(&resumed_text);
    assert!(!resumed.is_empty());
    assert!(resumed.iter().all(|event| event.id > resume_from));
    assert_eq!(resumed.first().unwrap().id, resume_from + 1);
    assert_eq!(resumed.last().unwrap().event, "run.completed");

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn cancelling_a_run_marks_it_cancelled_and_emits_final_sse_event() {
    let (endpoint, _state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_run_bridge: false,
        ..PoolStubOptions::default()
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

    let client = Client::new();
    let base_url = format!("http://{address}");
    let upload = upload_input(
        &client,
        &base_url,
        "input.csv",
        "text/csv",
        b"name,email\nalice,alice@example.com\n".to_vec(),
    )
    .await;
    let created = start_run(&client, &base_url, upload["filePath"].as_str().unwrap())
        .await
        .json::<Value>()
        .await
        .unwrap();
    let run_id = created["runId"].as_str().unwrap();

    let cancel = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/cancel"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(cancel.status(), reqwest::StatusCode::NO_CONTENT);

    let detail = wait_for_terminal_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}"),
        "cancelled",
    )
    .await;
    assert_eq!(detail["errorMessage"], "Run cancelled.");

    let events_text = client
        .get(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/events"
        ))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let events = parse_sse_events(&events_text);
    assert_eq!(events.last().unwrap().event, "run.completed");
    assert_eq!(
        serde_json::from_str::<Value>(&events.last().unwrap().data).unwrap()["finalStatus"],
        "cancelled"
    );

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn cancelling_before_bridge_ready_stops_the_session_attempt() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        run_bridge_delay_ms: 150,
        ..PoolStubOptions::default()
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

    let client = Client::new();
    let base_url = format!("http://{address}");
    let upload = upload_input(
        &client,
        &base_url,
        "input.csv",
        "text/csv",
        b"name,email\nalice,alice@example.com\n".to_vec(),
    )
    .await;
    let created = start_run(&client, &base_url, upload["filePath"].as_str().unwrap())
        .await
        .json::<Value>()
        .await
        .unwrap();
    let run_id = created["runId"].as_str().unwrap();

    tokio::time::sleep(Duration::from_millis(25)).await;
    let cancel = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/cancel"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(cancel.status(), reqwest::StatusCode::NO_CONTENT);

    let detail = wait_for_terminal_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}"),
        "cancelled",
    )
    .await;
    assert_eq!(detail["outputPath"], Value::Null);

    tokio::time::sleep(Duration::from_millis(250)).await;
    let state = state.lock().unwrap();
    assert!(state.artifact_download_urls.is_empty());
    assert!(state.artifact_upload_urls.is_empty());

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn cancelling_after_a_partial_result_clears_stale_output_state() {
    let (endpoint, _state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        run_execution_delay_ms: 200,
        ..PoolStubOptions::default()
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

    let client = Client::new();
    let base_url = format!("http://{address}");
    let upload = upload_input(
        &client,
        &base_url,
        "input.csv",
        "text/csv",
        b"name,email\nalice,alice@example.com\n".to_vec(),
    )
    .await;
    let created = start_run(&client, &base_url, upload["filePath"].as_str().unwrap())
        .await
        .json::<Value>()
        .await
        .unwrap();
    let run_id = created["runId"].as_str().unwrap();
    let detail_url =
        format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}");

    let partial = wait_for_output_path(&client, &detail_url).await;
    assert!(partial["outputPath"].is_string());

    let cancel = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/cancel"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(cancel.status(), reqwest::StatusCode::NO_CONTENT);

    let detail = wait_for_terminal_status(&client, &detail_url, "cancelled").await;
    assert_eq!(detail["outputPath"], Value::Null);
    assert_eq!(detail["validationIssues"], json!([]));
    assert_eq!(detail["errorMessage"], "Run cancelled.");

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn transient_run_bridge_startup_failures_retry_once_and_then_succeed() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        run_bridge_disconnect_before_ready_attempts: 1,
        ..PoolStubOptions::default()
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

    let client = Client::new();
    let base_url = format!("http://{address}");
    let upload = upload_input(
        &client,
        &base_url,
        "input.csv",
        "text/csv",
        b"name,email\nalice,alice@example.com\n".to_vec(),
    )
    .await;
    let created = start_run(&client, &base_url, upload["filePath"].as_str().unwrap())
        .await
        .json::<Value>()
        .await
        .unwrap();
    let run_id = created["runId"].as_str().unwrap();

    let detail = wait_for_terminal_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}"),
        "succeeded",
    )
    .await;
    assert!(detail["outputPath"].is_string());

    let state = state.lock().unwrap();
    assert_eq!(state.run_execution_count, 2);
    assert_eq!(state.artifact_download_urls.len(), 1);
    assert_eq!(state.artifact_upload_urls.len(), 1);

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn removed_public_file_routes_return_not_found() {
    let (endpoint, _state, stub_handle) = start_stub_server().await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );

    let files_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/workspaces/workspace-a/configs/config-v1/files")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(files_response.status(), StatusCode::NOT_FOUND);

    let executions_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/workspace-a/configs/config-v1/executions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(executions_response.status(), StatusCode::NOT_FOUND);

    stub_handle.abort();
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
    assert!(execution_codes[0].contains("websockets.sync.client"));
    assert!(!execution_codes[0].contains("capture_output=True"));

    let _ = socket.close(None).await;
    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn internal_bridge_route_can_only_attach_once() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_bridge: false,
        terminal_execution_delay_ms: 300,
        ..PoolStubOptions::default()
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
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_bridge: false,
        terminal_execution_delay_ms: 300,
        ..PoolStubOptions::default()
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

    browser_socket.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(connect_async(&bridge_url).await.is_err());

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn reconnect_while_previous_terminal_is_shutting_down_returns_clear_error() {
    let (endpoint, _state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_bridge: false,
        terminal_execution_delay_ms: 300,
        ..PoolStubOptions::default()
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
