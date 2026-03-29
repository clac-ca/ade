use std::{
    collections::HashMap,
    path::Path as FsPath,
    sync::{Arc, Mutex},
};

use ade_api::{
    readiness::{CreateReadinessControllerOptions, ReadinessController, ReadinessPhase},
    router::create_app,
    session::SessionService,
    state::AppState,
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
use serde_json::{Value, json};
use tempfile::tempdir;
use tower::util::ServiceExt;

const RUN_SENTINEL_PREFIX: &str = "__ADE_RUN_RESULT__=";

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
    state: Arc<Mutex<PoolStubState>>,
}

#[derive(serde::Deserialize)]
struct IdentifierQuery {
    #[serde(rename = "api-version")]
    #[allow(dead_code)]
    api_version: Option<String>,
    identifier: String,
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
        "directory": filename
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .unwrap_or("."),
        "name": filename
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(filename.as_str()),
        "sizeInBytes": bytes.len(),
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
    state.downloaded_names.push(filename.to_string());
    let key = format!("{}::{filename}", query.identifier);
    let content = state
        .uploaded_files
        .get(&key)
        .cloned()
        .unwrap_or_else(|| panic!("missing file: {filename}"));
    (StatusCode::OK, Bytes::from(content)).into_response()
}

async fn stub_execute(
    State(stub): State<PoolStub>,
    Query(query): Query<IdentifierQuery>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let code = body["code"].as_str().unwrap().to_string();
    let mut state = stub.state.lock().unwrap();
    state.identifiers.push(query.identifier);
    state.execution_codes.push(code.clone());

    let stdout = if code.contains(RUN_SENTINEL_PREFIX) {
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
    let state = Arc::new(Mutex::new(PoolStubState::default()));
    let stub = PoolStub {
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
    ]
    .into_iter()
    .collect();

    let session_service = SessionService::from_env(&env).unwrap();

    create_app(AppState {
        readiness: ready_state(),
        session_service,
        web_root: None,
    })
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
    assert_eq!(uploaded["filename"], "uploads/notes.txt");
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
    assert_eq!(
        listed,
        json!([{ "filename": "uploads/notes.txt", "size": 5 }])
    );

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
                .uri(
                    "/api/workspaces/workspace-a/configs/config-v1/files/uploads/notes.txt/content",
                )
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
                .uri("/api/workspaces/workspace-a/configs/config-v1/files/uploads/notes.txt")
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
    assert_eq!(
        payload,
        json!({
            "outputPath": "runs/run-1/output/input.normalized.xlsx",
            "validationIssues": [],
        })
    );
    assert!(payload.get("runId").is_none());
    assert!(payload.get("resultPath").is_none());

    {
        let state = state.lock().unwrap();
        assert_eq!(state.downloaded_names, Vec::<String>::new());
        assert!(
            state
                .uploaded_names
                .iter()
                .any(|name| name == "uploads/input.csv")
        );
        assert!(
            state
                .uploaded_names
                .iter()
                .any(|name| name == "session/engine/ade_engine-0.1.0-py3-none-any.whl")
        );
        assert!(
            state
                .uploaded_names
                .iter()
                .any(|name| name == "session/config/ade_config-0.1.0-py3-none-any.whl")
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
                .any(|name| name == "session/config/ade_config-0.2.0-py3-none-any.whl")
        );
    }

    handle.abort();
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
                        "inputPath": "uploads/input.csv",
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
