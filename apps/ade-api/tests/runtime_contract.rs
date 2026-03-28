use std::{
    collections::HashMap,
    path::Path as FsPath,
    sync::{Arc, Mutex},
};

use ade_api::{
    readiness::{CreateReadinessControllerOptions, ReadinessController, ReadinessPhase},
    router::create_app,
    runtime::RuntimeService,
    state::AppState,
    unix_time_ms,
};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Multipart, Path, Query, State},
    http::{Method, Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use tempfile::tempdir;
use tower::util::ServiceExt;

#[derive(Default)]
struct PoolStubState {
    uploaded_files: HashMap<String, Vec<u8>>,
    uploaded_names: Vec<String>,
    execution_codes: Vec<String>,
    identifiers: Vec<String>,
    mcp_invalid_environment_once: bool,
    mcp_python_launch_calls: usize,
    mcp_shell_launch_calls: usize,
    mcp_python_calls: usize,
    mcp_shell_calls: usize,
    stop_calls: usize,
}

#[derive(Clone)]
struct PoolStub {
    state: Arc<Mutex<PoolStubState>>,
}

#[derive(serde::Deserialize)]
struct IdentifierQuery {
    identifier: String,
    #[allow(dead_code)]
    #[serde(rename = "api-version")]
    api_version: Option<String>,
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
    let mut state = stub.state.lock().unwrap();
    state.identifiers.push(query.identifier.clone());
    state.uploaded_names.push(filename.clone());
    state
        .uploaded_files
        .insert(format!("{}::{filename}", query.identifier), bytes.to_vec());

    Json(json!({
        "properties": {
            "filename": filename,
            "size": bytes.len(),
        }
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
                "properties": {
                    "filename": filename,
                    "size": content.len(),
                }
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "value": files }))
}

async fn stub_download_file(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
    Path(filename): Path<String>,
) -> impl IntoResponse {
    let mut state = stub.state.lock().unwrap();
    state.identifiers.push(query.identifier.clone());
    let key = format!("{}::{filename}", query.identifier);
    let content = state.uploaded_files.get(&key).unwrap().clone();
    (StatusCode::OK, Bytes::from(content))
}

async fn stub_execute(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let mut state = stub.state.lock().unwrap();
    state.identifiers.push(query.identifier);
    state
        .execution_codes
        .push(body["properties"]["code"].as_str().unwrap().to_string());
    (
        [
            ("operation-id", "op-1"),
            ("x-ms-session-guid", "cfg-session"),
        ],
        Json(json!({
            "properties": {
                "stdout": "ok",
                "stderr": "",
                "exitCode": 0,
                "status": "Succeeded",
            }
        })),
    )
}

async fn stub_stop_session(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
) -> impl IntoResponse {
    let mut state = stub.state.lock().unwrap();
    state.identifiers.push(query.identifier);
    state.stop_calls += 1;
    Json(json!({}))
}

async fn stub_mcp(State(stub): State<PoolStub>, Json(body): Json<Value>) -> impl IntoResponse {
    match body["method"].as_str().unwrap() {
        "initialize" => Json(json!({
            "jsonrpc": "2.0",
            "id": body["id"].clone(),
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": { "list": true, "call": true } }
            }
        })),
        "tools/list" => Json(json!({
            "jsonrpc": "2.0",
            "id": body["id"].clone(),
            "result": {
                "tools": [
                    {"name": "launchPythonEnvironment"},
                    {"name": "launchShell"},
                    {"name": "runShellCommandInRemoteEnvironment"},
                    {"name": "runPythonCodeInRemoteEnvironment"}
                ]
            }
        })),
        "tools/call" => {
            let name = body["params"]["name"].as_str().unwrap();
            if name == "launchPythonEnvironment" {
                stub.state.lock().unwrap().mcp_python_launch_calls += 1;
                return Json(json!({
                    "jsonrpc": "2.0",
                    "id": body["id"].clone(),
                    "result": { "structuredContent": { "environmentId": "env-py-123" } }
                }));
            }
            if name == "launchShell" {
                stub.state.lock().unwrap().mcp_shell_launch_calls += 1;
                return Json(json!({
                    "jsonrpc": "2.0",
                    "id": body["id"].clone(),
                    "result": { "structuredContent": { "environmentId": "env-123" } }
                }));
            }
            if name == "runPythonCodeInRemoteEnvironment" {
                let mut state = stub.state.lock().unwrap();
                state.mcp_python_calls += 1;
                assert_eq!(
                    body["params"]["arguments"]["environmentId"],
                    json!("env-py-123")
                );
                return Json(json!({
                    "jsonrpc": "2.0",
                    "id": body["id"].clone(),
                    "result": {
                        "structuredContent": {
                            "stdout": "python-ok",
                            "stderr": ""
                        }
                    }
                }));
            }
            if name == "runShellCommandInRemoteEnvironment" {
                let mut state = stub.state.lock().unwrap();
                state.mcp_shell_calls += 1;
                if state.mcp_invalid_environment_once {
                    state.mcp_invalid_environment_once = false;
                    return Json(json!({
                        "jsonrpc": "2.0",
                        "id": body["id"].clone(),
                        "error": { "code": -32000, "message": "Environment not found: env-123" }
                    }));
                }
                assert_eq!(
                    body["params"]["arguments"]["environmentId"],
                    json!("env-123")
                );
                return Json(json!({
                    "jsonrpc": "2.0",
                    "id": body["id"].clone(),
                    "result": {
                        "structuredContent": {
                            "stdout": "hello",
                            "stderr": "",
                            "exitCode": 0
                        }
                    }
                }));
            }
            Json(json!({
                "jsonrpc": "2.0",
                "id": body["id"].clone(),
                "error": { "code": -32000, "message": "Unsupported tool" }
            }))
        }
        _ => Json(json!({
            "jsonrpc": "2.0",
            "id": body["id"].clone(),
            "error": { "code": -32000, "message": "Unsupported method" }
        })),
    }
}

async fn start_stub_server() -> (
    String,
    Arc<Mutex<PoolStubState>>,
    tokio::task::JoinHandle<()>,
) {
    let state = Arc::new(Mutex::new(PoolStubState::default()));
    let stub = PoolStub {
        state: state.clone(),
    };
    let app = Router::new()
        .route("/files/upload", post(stub_upload_file))
        .route("/files", get(stub_list_files))
        .route("/files/content/{filename}", get(stub_download_file))
        .route("/code/execute", post(stub_execute))
        .route("/.management/stopSession", post(stub_stop_session))
        .route("/mcp", post(stub_mcp))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{address}"), state, handle)
}

fn app_with_runtime(
    endpoint: &str,
    config_wheel_path: &FsPath,
    engine_wheel_path: &FsPath,
) -> axum::Router {
    let env = [
        (
            "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT".to_string(),
            endpoint.to_string(),
        ),
        (
            "ADE_SESSION_POOL_MCP_ENDPOINT".to_string(),
            format!("{endpoint}/mcp"),
        ),
        (
            "ADE_ACTIVE_CONFIG_WHEEL_PATH".to_string(),
            config_wheel_path.display().to_string(),
        ),
        (
            "ADE_ENGINE_WHEEL_PATH".to_string(),
            engine_wheel_path.display().to_string(),
        ),
    ]
    .into_iter()
    .collect();
    let service = RuntimeService::create_from_env(&env, false)
        .unwrap()
        .expect("runtime configured");

    create_app(AppState {
        readiness: ready_state(),
        runtime: Some(service),
        web_root: None,
    })
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn multipart_request(uri: &str, filename: &str, content: &[u8]) -> Request<Body> {
    let boundary = "ade-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn executions_route_uploads_the_active_config_wheel_and_wraps_code() {
    let temp_dir = tempdir().unwrap();
    let config_wheel_path = temp_dir.path().join("ade_config-0.1.0-py3-none-any.whl");
    let engine_wheel_path = temp_dir.path().join("ade_engine-0.1.0-py3-none-any.whl");
    std::fs::write(&config_wheel_path, b"config-wheel-bytes").unwrap();
    std::fs::write(&engine_wheel_path, b"engine-wheel-bytes").unwrap();
    let (endpoint, state, handle) = start_stub_server().await;
    let app = app_with_runtime(&endpoint, &config_wheel_path, &engine_wheel_path);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/runtime/code/execute")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"properties":{"codeInputType":"inline","executionType":"synchronous","code":"print('hello')"}} "#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("operation-id").unwrap(), "op-1");

    let payload = json_body(response).await;
    assert_eq!(payload["properties"]["stdout"], "ok");

    let state = state.lock().unwrap();
    assert_eq!(
        state.uploaded_names,
        vec![
            "ade_engine-0.1.0-py3-none-any.whl",
            "ade_config-0.1.0-py3-none-any.whl",
        ]
    );
    assert!(
        state
            .identifiers
            .iter()
            .all(|identifier| identifier.starts_with("cfg-"))
    );
    assert!(state.execution_codes[0].contains("import importlib.metadata"));
    assert!(state.execution_codes[0].contains("ade-engine"));
    assert!(state.execution_codes[0].contains("ade-config"));
    assert!(state.execution_codes[0].contains("print('hello')"));

    handle.abort();
}

#[tokio::test]
async fn files_routes_proxy_upload_list_metadata_and_download() {
    let temp_dir = tempdir().unwrap();
    let config_wheel_path = temp_dir.path().join("ade_config-0.1.0-py3-none-any.whl");
    let engine_wheel_path = temp_dir.path().join("ade_engine-0.1.0-py3-none-any.whl");
    std::fs::write(&config_wheel_path, b"config-wheel-bytes").unwrap();
    std::fs::write(&engine_wheel_path, b"engine-wheel-bytes").unwrap();
    let (endpoint, _state, handle) = start_stub_server().await;
    let app = app_with_runtime(&endpoint, &config_wheel_path, &engine_wheel_path);

    let upload_response = app
        .clone()
        .oneshot(multipart_request(
            "/api/runtime/files/upload",
            "input.csv",
            b"name,email\nalice,A@example.com\n",
        ))
        .await
        .unwrap();
    assert_eq!(upload_response.status(), StatusCode::OK);

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/runtime/files")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let list_payload = json_body(list_response).await;
    assert_eq!(
        list_payload["value"][0]["properties"]["filename"],
        "input.csv"
    );

    let download_response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/runtime/files/content/input.csv")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(download_response.status(), StatusCode::OK);
    let bytes = to_bytes(download_response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), b"name,email\nalice,A@example.com\n");

    handle.abort();
}

#[tokio::test]
async fn mcp_route_caches_environment_ids_until_stop_session() {
    let temp_dir = tempdir().unwrap();
    let config_wheel_path = temp_dir.path().join("ade_config-0.1.0-py3-none-any.whl");
    let engine_wheel_path = temp_dir.path().join("ade_engine-0.1.0-py3-none-any.whl");
    std::fs::write(&config_wheel_path, b"config-wheel-bytes").unwrap();
    std::fs::write(&engine_wheel_path, b"engine-wheel-bytes").unwrap();
    let (endpoint, state, handle) = start_stub_server().await;
    let app = app_with_runtime(&endpoint, &config_wheel_path, &engine_wheel_path);

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/runtime/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":"1","method":"tools/call","params":{"name":"runShellCommandInRemoteEnvironment","arguments":{"shellCommand":"echo hello"}}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = json_body(response).await;
        assert_eq!(payload["result"]["structuredContent"]["stdout"], "hello");
    }

    let stop_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/runtime/.management/stopSession")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stop_response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/runtime/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":"1","method":"tools/call","params":{"name":"runShellCommandInRemoteEnvironment","arguments":{"shellCommand":"echo hello"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let state = state.lock().unwrap();
    assert_eq!(state.mcp_shell_launch_calls, 2);
    assert_eq!(state.mcp_shell_calls, 3);
    assert_eq!(state.stop_calls, 1);

    handle.abort();
}

#[tokio::test]
async fn mcp_route_relaunches_when_the_cached_environment_is_invalid() {
    let temp_dir = tempdir().unwrap();
    let config_wheel_path = temp_dir.path().join("ade_config-0.1.0-py3-none-any.whl");
    let engine_wheel_path = temp_dir.path().join("ade_engine-0.1.0-py3-none-any.whl");
    std::fs::write(&config_wheel_path, b"config-wheel-bytes").unwrap();
    std::fs::write(&engine_wheel_path, b"engine-wheel-bytes").unwrap();
    let (endpoint, state, handle) = start_stub_server().await;
    state.lock().unwrap().mcp_invalid_environment_once = true;
    let app = app_with_runtime(&endpoint, &config_wheel_path, &engine_wheel_path);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/runtime/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":"1","method":"tools/call","params":{"name":"runShellCommandInRemoteEnvironment","arguments":{"shellCommand":"echo hello"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let payload = json_body(response).await;
    assert_eq!(payload["result"]["structuredContent"]["stdout"], "hello");

    let state = state.lock().unwrap();
    assert_eq!(state.mcp_shell_launch_calls, 2);
    assert_eq!(state.mcp_shell_calls, 2);

    handle.abort();
}

#[tokio::test]
async fn mcp_route_caches_python_environment_ids() {
    let temp_dir = tempdir().unwrap();
    let config_wheel_path = temp_dir.path().join("ade_config-0.1.0-py3-none-any.whl");
    let engine_wheel_path = temp_dir.path().join("ade_engine-0.1.0-py3-none-any.whl");
    std::fs::write(&config_wheel_path, b"config-wheel-bytes").unwrap();
    std::fs::write(&engine_wheel_path, b"engine-wheel-bytes").unwrap();
    let (endpoint, state, handle) = start_stub_server().await;
    let app = app_with_runtime(&endpoint, &config_wheel_path, &engine_wheel_path);

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/runtime/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":"1","method":"tools/call","params":{"name":"runPythonCodeInRemoteEnvironment","arguments":{"pythonCode":"print('hello')"}}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = json_body(response).await;
        assert_eq!(
            payload["result"]["structuredContent"]["stdout"],
            "python-ok"
        );
    }

    let state = state.lock().unwrap();
    assert_eq!(state.mcp_python_launch_calls, 1);
    assert_eq!(state.mcp_python_calls, 2);

    handle.abort();
}
