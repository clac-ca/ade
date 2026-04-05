use std::{
    collections::HashMap,
    path::Path as FsPath,
    sync::{Arc, Mutex},
    time::Duration,
};

use ade_api::{
    api::{AppState, create_app},
    readiness::{DatabaseReadiness, ReadinessController, ReadinessPhase, ReadinessSnapshot},
    runs::{InMemoryRunStore, RunService},
    sandbox_environment::SandboxEnvironmentManager,
    terminal::TerminalService,
    unix_time_ms,
};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, post},
};
use flate2::{Compression, write::GzEncoder};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};
use tar::Builder;
use tempfile::tempdir;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tower::util::ServiceExt;

const AZURE_SHELL_API_VERSION: &str = "2025-10-02-preview";
const LOCAL_SESSION_POOL_BEARER_TOKEN: &str = "ade-local-session-token";

#[derive(Default)]
struct PoolStubState {
    blobs: HashMap<String, StubBlobObject>,
    execution_codes: Vec<String>,
    identifiers: Vec<String>,
    run_execution_count: usize,
    session_files: HashMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StubBlobObject {
    content_type: String,
    bytes: Vec<u8>,
}

#[derive(Clone)]
struct PoolStub {
    options: PoolStubOptions,
    state: Arc<Mutex<PoolStubState>>,
}

#[derive(Clone, Copy)]
struct PoolStubOptions {
    auto_connect_run_connector: bool,
    auto_connect_terminal_connector: bool,
    run_connector_delay_ms: u64,
    run_connector_disconnect_before_ready_attempts: usize,
    run_execution_delay_ms: u64,
    terminal_connector_delay_ms: u64,
    terminal_execution_delay_ms: u64,
}

impl Default for PoolStubOptions {
    fn default() -> Self {
        Self {
            auto_connect_run_connector: true,
            auto_connect_terminal_connector: true,
            run_connector_delay_ms: 0,
            run_connector_disconnect_before_ready_attempts: 0,
            run_execution_delay_ms: 0,
            terminal_connector_delay_ms: 0,
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

fn write_sandbox_environment_archive(archive_path: &FsPath, base_wheel_path: &FsPath) {
    let file = std::fs::File::create(archive_path).unwrap();
    let encoder = GzEncoder::new(file, Compression::default());
    let mut archive = Builder::new(encoder);

    let append_bytes = |archive: &mut Builder<GzEncoder<std::fs::File>>,
                        archive_path: &str,
                        content: &[u8],
                        mode: u32| {
        let mut header = tar::Header::new_gnu();
        header.set_path(archive_path).unwrap();
        header.set_mode(mode);
        header.set_size(content.len() as u64);
        header.set_cksum();
        archive.append(&header, content).unwrap();
    };

    append_bytes(
        &mut archive,
        "app/ade/bin/reverse-connect",
        b"connector-binary",
        0o755,
    );
    append_bytes(
        &mut archive,
        "app/ade/bin/setup.sh",
        b"#!/bin/sh\nexit 0\n",
        0o755,
    );
    append_bytes(
        &mut archive,
        "app/ade/python/current/bin/python3",
        b"python3",
        0o755,
    );
    append_bytes(
        &mut archive,
        "app/ade/python/current/bin/ade",
        b"ade",
        0o755,
    );

    let base_wheel_bytes = std::fs::read(base_wheel_path).unwrap();
    let base_wheel_name = base_wheel_path.file_name().unwrap().to_str().unwrap();
    append_bytes(
        &mut archive,
        &format!("app/ade/wheelhouse/base/{base_wheel_name}"),
        &base_wheel_bytes,
        0o644,
    );

    archive.finish().unwrap();
    let encoder = archive.into_inner().unwrap();
    encoder.finish().unwrap();
}

async fn stub_upload_file(
    State(stub): State<PoolStub>,
    headers: HeaderMap,
    Query(query): Query<IdentifierQuery>,
    mut multipart: Multipart,
) -> Response {
    let result = async {
        validate_stub_request(&headers, query.api_version.as_deref())?;
        let field = multipart
            .next_field()
            .await
            .unwrap()
            .expect("multipart file");
        let filename = validate_stub_filename(field.file_name().expect("multipart filename"))?;
        let bytes = field.bytes().await.unwrap();
        let stored_name = match normalize_stub_directory(query.path.as_deref())? {
            Some(path) => format!("{path}/{filename}"),
            None => filename.clone(),
        };
        let mut state = stub.state.lock().unwrap();
        state.identifiers.push(query.identifier.clone());
        state.session_files.insert(
            session_file_storage_key(&query.identifier, &stored_name),
            bytes.to_vec(),
        );

        Ok::<_, String>(Json(json!({
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
        })))
    }
    .await;

    match result {
        Ok(payload) => payload.into_response(),
        Err(message) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": message,
            })),
        )
            .into_response(),
    }
}

async fn stub_download_file(
    State(stub): State<PoolStub>,
    headers: HeaderMap,
    Path(filename): Path<String>,
    Query(query): Query<IdentifierQuery>,
) -> Response {
    let result = (|| {
        validate_stub_request(&headers, query.api_version.as_deref())?;
        let filename = validate_stub_filename(&filename)?;
        let stored_name = match normalize_stub_directory(query.path.as_deref())? {
            Some(path) => format!("{path}/{filename}"),
            None => filename,
        };
        let key = session_file_storage_key(&query.identifier, &stored_name);
        let Some(bytes) = stub.state.lock().unwrap().session_files.get(&key).cloned() else {
            return Err("File not found.".to_string());
        };
        Ok::<_, String>(bytes)
    })();

    match result {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(message) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": message,
            })),
        )
            .into_response(),
    }
}

async fn stub_execute(
    State(stub): State<PoolStub>,
    headers: HeaderMap,
    Query(query): Query<IdentifierQuery>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if let Err(message) = validate_stub_request(&headers, query.api_version.as_deref()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": message,
            })),
        )
            .into_response();
    }
    let code = body
        .get("shellCommand")
        .and_then(Value::as_str)
        .expect("session pool execution should use shellCommand")
        .to_string();
    let identifier = query.identifier.clone();
    {
        let mut state = stub.state.lock().unwrap();
        state.identifiers.push(identifier.clone());
        state.execution_codes.push(code.clone());
    }

    let connector_url = extract_connector_url(&code);
    let bearer_token = extract_bearer_token(&code);
    if code.contains("reverse-connect")
        && let (Some(connector_url), Some(bearer_token)) = (connector_url, bearer_token)
    {
        let run_execution_count = {
            let mut state = stub.state.lock().unwrap();
            state.run_execution_count += 1;
            state.run_execution_count
        };
        let connect_delay_ms = stub
            .options
            .run_connector_delay_ms
            .max(stub.options.terminal_connector_delay_ms);
        let execution_delay_ms = stub
            .options
            .run_execution_delay_ms
            .max(stub.options.terminal_execution_delay_ms);

        if run_execution_count <= stub.options.run_connector_disconnect_before_ready_attempts {
            if let Ok((mut socket, _response)) =
                connect_async(connector_request(&connector_url, &bearer_token)).await
            {
                let _ = socket.close(None).await;
            }
            return Json(json!({
                "status": "0",
                "result": {
                    "stdout": "",
                    "stderr": "",
                    "executionTimeInMilliseconds": 4,
                }
            }))
            .into_response();
        }

        if connect_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(connect_delay_ms)).await;
        }

        if stub.options.auto_connect_run_connector && stub.options.auto_connect_terminal_connector {
            drive_reverse_connect_stub(
                stub.clone(),
                &identifier,
                &connector_url,
                &bearer_token,
                execution_delay_ms,
            )
            .await;
        } else if execution_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(execution_delay_ms)).await;
        }

        return Json(json!({
            "status": "0",
            "result": {
                "stdout": "",
                "stderr": "",
                "executionTimeInMilliseconds": 4,
            }
        }))
        .into_response();
    }

    Json(json!({
        "status": "0",
        "result": {
            "stdout": "ok\n",
            "stderr": "",
            "executionTimeInMilliseconds": 4,
        }
    }))
    .into_response()
}

fn blob_storage_key(account: &str, container: &str, blob_path: &str) -> String {
    format!(
        "{account}/{container}/{}",
        blob_path.trim_start_matches('/')
    )
}

fn session_file_storage_key(identifier: &str, path: &str) -> String {
    format!("{identifier}:{}", path.trim_start_matches('/'))
}

fn normalize_stub_directory(path: Option<&str>) -> Result<Option<String>, String> {
    let Some(path) = path
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != ".")
    else {
        return Ok(None);
    };
    let normalized = path.trim_matches('/');
    if normalized.is_empty() {
        return Ok(None);
    }
    for segment in normalized.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains('.')
            || segment
                .chars()
                .any(|character| !is_allowed_stub_path_character(character))
        {
            return Err(format!(
                "File Path '/{normalized}' is invalid because 'path cannot contain any reserved file path characters'."
            ));
        }
    }
    Ok(Some(normalized.to_string()))
}

fn validate_stub_request(headers: &HeaderMap, api_version: Option<&str>) -> Result<(), String> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "Missing Authorization header.".to_string())?;
    if authorization != format!("Bearer {LOCAL_SESSION_POOL_BEARER_TOKEN}") {
        return Err("Unexpected Authorization header.".to_string());
    }
    if api_version != Some(AZURE_SHELL_API_VERSION) {
        return Err(format!(
            "Unexpected api-version. Expected '{AZURE_SHELL_API_VERSION}'."
        ));
    }
    Ok(())
}

fn validate_stub_filename(filename: &str) -> Result<String, String> {
    let trimmed = filename.trim().trim_matches('/');
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed
            .chars()
            .any(|character| !is_allowed_stub_file_character(character))
    {
        return Err(format!(
            "File Name '{trimmed}' is invalid because 'filename cannot contain any reserved file path characters'."
        ));
    }
    Ok(trimmed.to_string())
}

fn is_allowed_stub_file_character(character: char) -> bool {
    character.is_alphanumeric()
        || character == ' '
        || matches!(
            character,
            '-' | '_' | '.' | '@' | '$' | '&' | '=' | ';' | ',' | '#' | '%' | '^' | '(' | ')'
        )
}

fn is_allowed_stub_path_character(character: char) -> bool {
    is_allowed_stub_file_character(character) && character != '.'
}

fn connector_request(
    connector_url: &str,
    bearer_token: &str,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = connector_url.into_client_request().unwrap();
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {bearer_token}")).unwrap(),
    );
    request.headers_mut().insert(
        "sec-websocket-protocol",
        HeaderValue::from_static(::reverse_connect::protocol::WEBSOCKET_SUBPROTOCOL),
    );
    request
}

fn stored_blob(state: &PoolStubState, path: &str) -> StubBlobObject {
    state
        .blobs
        .get(&blob_storage_key("devstoreaccount1", "documents", path))
        .cloned()
        .unwrap_or_else(|| panic!("missing blob for path: {path}"))
}

async fn stub_blob_account(Query(query): Query<HashMap<String, String>>) -> impl IntoResponse {
    if query.get("restype").map(String::as_str) == Some("service")
        && query.get("comp").map(String::as_str) == Some("properties")
    {
        return StatusCode::ACCEPTED;
    }

    StatusCode::NOT_FOUND
}

async fn stub_blob_container(Query(query): Query<HashMap<String, String>>) -> impl IntoResponse {
    if query.get("restype").map(String::as_str) == Some("container") {
        return StatusCode::CREATED;
    }

    StatusCode::NOT_FOUND
}

async fn stub_blob_object(
    State(stub): State<PoolStub>,
    Path((account, container, blob_path)): Path<(String, String, String)>,
    method: axum::http::Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let key = blob_storage_key(&account, &container, &blob_path);
    match method {
        axum::http::Method::PUT => {
            let content_type = headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            stub.state.lock().unwrap().blobs.insert(
                key,
                StubBlobObject {
                    bytes: body.to_vec(),
                    content_type,
                },
            );
            StatusCode::CREATED.into_response()
        }
        axum::http::Method::GET => {
            let Some(blob) = stub.state.lock().unwrap().blobs.get(&key).cloned() else {
                return StatusCode::NOT_FOUND.into_response();
            };
            ([(header::CONTENT_TYPE, blob.content_type)], blob.bytes).into_response()
        }
        axum::http::Method::HEAD => {
            let Some(blob) = stub.state.lock().unwrap().blobs.get(&key).cloned() else {
                return StatusCode::NOT_FOUND.into_response();
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, blob.content_type)
                .header(header::CONTENT_LENGTH, blob.bytes.len().to_string())
                .body(Body::empty())
                .unwrap()
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
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
        .route(
            "/files/{filename}/content",
            axum::routing::get(stub_download_file),
        )
        .route("/executions", post(stub_execute))
        .route("/{account}", any(stub_blob_account))
        .route("/{account}/{container}", any(stub_blob_container))
        .route("/{account}/{container}/{*blob_path}", any(stub_blob_object))
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
    _config_targets: &[(&str, &str, &FsPath)],
    base_wheel_path: &FsPath,
) -> axum::Router {
    app_with_session_and_url_and_run_limit(
        endpoint,
        "http://127.0.0.1:8000",
        &[],
        base_wheel_path,
        None,
    )
}

fn app_with_session_and_url(
    endpoint: &str,
    app_url: &str,
    _config_targets: &[(&str, &str, &FsPath)],
    base_wheel_path: &FsPath,
) -> axum::Router {
    app_with_session_and_url_and_run_limit(endpoint, app_url, &[], base_wheel_path, None)
}

fn app_with_session_and_url_and_run_limit(
    endpoint: &str,
    app_url: &str,
    _config_targets: &[(&str, &str, &FsPath)],
    base_wheel_path: &FsPath,
    run_max_concurrent: Option<usize>,
) -> axum::Router {
    let runtime_dir = tempdir().unwrap();
    let environment_archive = runtime_dir.path().join("sandbox-environment.tar.gz");
    write_sandbox_environment_archive(&environment_archive, base_wheel_path);
    let env: std::collections::BTreeMap<String, String> = [
        (
            "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT".to_string(),
            endpoint.to_string(),
        ),
        (
            "ADE_SESSION_POOL_BEARER_TOKEN".to_string(),
            LOCAL_SESSION_POOL_BEARER_TOKEN.to_string(),
        ),
        (
            "ADE_SANDBOX_ENVIRONMENT_SECRET".to_string(),
            "test-session-secret".to_string(),
        ),
        ("ADE_PUBLIC_API_URL".to_string(), app_url.to_string()),
        (
            "ADE_BLOB_ACCOUNT_URL".to_string(),
            format!("{endpoint}/devstoreaccount1"),
        ),
        (
            "ADE_BLOB_CONTAINER".to_string(),
            "documents".to_string(),
        ),
        (
            "ADE_BLOB_ACCOUNT_KEY".to_string(),
            "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=="
                .to_string(),
        ),
    ]
    .into_iter()
    .collect();

    std::mem::forget(runtime_dir);

    let sandbox_environment_manager = Arc::new(
        SandboxEnvironmentManager::from_paths(
            app_url,
            endpoint,
            Some(LOCAL_SESSION_POOL_BEARER_TOKEN),
            "test-session-secret",
            environment_archive,
        )
        .unwrap(),
    );
    let mut run_env = env.clone();
    if let Some(run_max_concurrent) = run_max_concurrent {
        run_env.insert(
            "ADE_RUN_MAX_CONCURRENT".to_string(),
            run_max_concurrent.to_string(),
        );
    }
    let run_service = Arc::new(
        RunService::from_env(
            &run_env,
            Arc::clone(&sandbox_environment_manager),
            Arc::new(InMemoryRunStore::default()),
        )
        .unwrap(),
    );
    let terminal_service = Arc::new(
        TerminalService::from_env(&env, Arc::clone(&sandbox_environment_manager)).unwrap(),
    );

    create_app(AppState {
        readiness: ready_state(),
        sandbox_environment_manager,
        run_service,
        terminal_service,
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

fn extract_launch_arg(code: &str, flag: &str) -> Option<String> {
    let (_, rest) = code.split_once(&format!("{flag} "))?;
    if let Some(rest) = rest.strip_prefix('\'') {
        let end = rest.find('\'')?;
        return Some(rest[..end].to_string());
    }
    Some(rest.split_whitespace().next()?.to_string())
}

fn extract_bearer_token(code: &str) -> Option<String> {
    extract_launch_arg(code, "--bearer-token")
}

fn extract_connector_url(code: &str) -> Option<String> {
    extract_launch_arg(code, "--url")
}

async fn drive_reverse_connect_stub(
    stub: PoolStub,
    identifier: &str,
    connector_url: &str,
    bearer_token: &str,
    execution_delay_ms: u64,
) {
    if let Ok((mut socket, _response)) =
        connect_async(connector_request(connector_url, bearer_token)).await
    {
        let hello = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "connector.hello",
            "params": {
                "capabilities": ["exec", "pty"],
                "host": {
                    "arch": "x86_64",
                    "os": "linux"
                },
                "version": "test"
            }
        });
        let _ = socket.send(Message::Text(hello.to_string().into())).await;

        let _ = socket.next().await;

        while let Some(Ok(message)) = socket.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            let payload = serde_json::from_str::<Value>(&text).unwrap();
            match payload["method"].as_str() {
                Some("channel.open") => {
                    let id = payload["id"].as_i64().unwrap();
                    let params = payload["params"].as_object().unwrap();
                    let channel_id = params["channelId"].as_str().unwrap();
                    let kind = params["kind"].as_str().unwrap();
                    let _ = socket
                        .send(Message::Text(
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {},
                            })
                            .to_string()
                            .into(),
                        ))
                        .await;

                    if kind == "pty" {
                        let output = json!({
                            "jsonrpc": "2.0",
                            "method": "channel.data",
                            "params": {
                                "channelId": channel_id,
                                "stream": "pty",
                                "data": "dGVybWluYWwtb2sNCg==",
                            }
                        });
                        let exit = json!({
                            "jsonrpc": "2.0",
                            "method": "channel.exit",
                            "params": {
                                "channelId": channel_id,
                                "code": 0,
                            }
                        });
                        let _ = socket.send(Message::Text(output.to_string().into())).await;
                        let _ = socket.send(Message::Text(exit.to_string().into())).await;
                        continue;
                    }

                    let command = params["command"].as_str().unwrap_or_default();
                    if command.starts_with("sh ") {
                        let exit = json!({
                            "jsonrpc": "2.0",
                            "method": "channel.exit",
                            "params": {
                                "channelId": channel_id,
                                "code": 0,
                            }
                        });
                        let _ = socket.send(Message::Text(exit.to_string().into())).await;
                        continue;
                    }
                    if command.contains("-m pip install") {
                        let exit = json!({
                            "jsonrpc": "2.0",
                            "method": "channel.exit",
                            "params": {
                                "channelId": channel_id,
                                "code": 0,
                            }
                        });
                        let _ = socket.send(Message::Text(exit.to_string().into())).await;
                        continue;
                    }

                    let command_args = single_quoted_args(command);
                    let input_path = command_args
                        .get(1)
                        .expect("run command input path")
                        .to_string();
                    let output_dir = command_args
                        .get(2)
                        .expect("run command output dir")
                        .to_string();
                    let input_filename = std::path::Path::new(&input_path)
                        .file_name()
                        .and_then(|value| value.to_str())
                        .expect("run input filename");
                    let output_filename = format!(
                        "{}.normalized.xlsx",
                        std::path::Path::new(input_filename)
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .expect("run input stem")
                    );
                    let output_storage_key = session_file_storage_key(
                        identifier,
                        format!("{}/{}", output_dir.trim_start_matches('/'), output_filename)
                            .as_str(),
                    );
                    let stdout = json!({
                        "jsonrpc": "2.0",
                        "method": "channel.data",
                        "params": {
                            "channelId": channel_id,
                            "stream": "stdout",
                            "data": "TG9hZGVkIDEyIHJvd3MK",
                        }
                    });
                    let exit = json!({
                        "jsonrpc": "2.0",
                        "method": "channel.exit",
                        "params": {
                            "channelId": channel_id,
                            "code": 0,
                        }
                    });
                    let _ = socket.send(Message::Text(stdout.to_string().into())).await;
                    if execution_delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(execution_delay_ms)).await;
                    }
                    stub.state
                        .lock()
                        .unwrap()
                        .session_files
                        .insert(output_storage_key, b"normalized-output".to_vec());
                    let _ = socket.send(Message::Text(exit.to_string().into())).await;
                    continue;
                }
                Some("channel.close") | Some("session.shutdown") => break,
                _ => {}
            }
        }

        let _ = socket.close(None).await;
    }
}

fn resolve_url(base_url: &str, url: &str) -> String {
    if Url::parse(url).is_ok() {
        return url.to_string();
    }

    Url::parse(base_url).unwrap().join(url).unwrap().to_string()
}

fn single_quoted_args(command: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut remainder = command;
    while let Some(start) = remainder.find('\'') {
        let after_start = &remainder[start + 1..];
        let Some(end) = after_start.find('\'') else {
            break;
        };
        values.push(after_start[..end].to_string());
        remainder = &after_start[end + 1..];
    }
    values
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

async fn create_run_download(
    client: &Client,
    base_url: &str,
    run_id: &str,
    artifact: &str,
) -> Value {
    let response = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/downloads"
        ))
        .json(&json!({
            "artifact": artifact,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response.json::<Value>().await.unwrap()
}

async fn download_run_artifact(
    client: &Client,
    base_url: &str,
    run_id: &str,
    artifact: &str,
) -> (Value, Vec<u8>) {
    let payload = create_run_download(client, base_url, run_id, artifact).await;
    let download = payload["download"].as_object().unwrap();
    let method = Method::from_bytes(download["method"].as_str().unwrap().as_bytes()).unwrap();
    let mut request = client.request(
        method,
        resolve_url(base_url, download["url"].as_str().unwrap()),
    );
    for (name, value) in download["headers"].as_object().unwrap() {
        let name = HeaderName::from_bytes(name.as_bytes()).unwrap();
        let value = HeaderValue::from_str(value.as_str().unwrap()).unwrap();
        request = request.header(name, value);
    }
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let bytes = response.bytes().await.unwrap().to_vec();
    (payload, bytes)
}

async fn wait_for_run_status(client: &Client, url: &str, expected: &str) -> Value {
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

async fn wait_for_run_execution_count(state: &Arc<Mutex<PoolStubState>>, expected: usize) {
    for _ in 0..40 {
        if state.lock().unwrap().run_execution_count == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    panic!(
        "run execution count never reached {expected}: {}",
        state.lock().unwrap().run_execution_count
    );
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
        payload["upload"]["headers"]["content-type"],
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );
    assert!(payload["upload"]["headers"]["x-ms-blob-type"].is_string());
    assert!(payload["upload"]["headers"]["x-ms-version"].is_string());
    let upload_url = Url::parse(payload["upload"]["url"].as_str().unwrap()).unwrap();
    assert_eq!(upload_url.host_str(), Some("127.0.0.1"));
    assert!(upload_url.path().contains("/devstoreaccount1/documents/"));
    assert_eq!(
        upload_url.query_pairs().find(|(name, _)| name == "sp"),
        Some(("sp".into(), "cw".into()))
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
async fn bulk_upload_batches_return_direct_upload_instructions_per_file() {
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
                .uri("/api/workspaces/workspace-a/configs/config-v1/uploads/batches")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "files": [
                            {
                                "filename": "../alpha.csv",
                                "contentType": "text/csv",
                                "size": 10,
                            },
                            {
                                "filename": "reports/beta.xlsx",
                                "contentType": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                                "size": 20,
                            }
                        ],
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
    let batch_id = payload["batchId"].as_str().unwrap();
    assert!(batch_id.starts_with("bat_"));
    let items = payload["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items[0]["fileId"].as_str().unwrap().starts_with("fil_"));
    assert!(items[1]["fileId"].as_str().unwrap().starts_with("fil_"));
    assert_ne!(items[0]["fileId"], items[1]["fileId"]);
    assert_eq!(
        items[0]["filePath"],
        Value::String(format!(
            "workspaces/workspace-a/configs/config-v1/uploads/batches/{batch_id}/{}/alpha.csv",
            items[0]["fileId"].as_str().unwrap()
        ))
    );
    assert_eq!(
        items[1]["filePath"],
        Value::String(format!(
            "workspaces/workspace-a/configs/config-v1/uploads/batches/{batch_id}/{}/beta.xlsx",
            items[1]["fileId"].as_str().unwrap()
        ))
    );
    assert_eq!(items[0]["upload"]["method"], "PUT");
    assert_eq!(items[0]["upload"]["headers"]["content-type"], "text/csv");
    assert!(items[0]["upload"]["headers"]["x-ms-blob-type"].is_string());
    let upload_url = Url::parse(items[0]["upload"]["url"].as_str().unwrap()).unwrap();
    assert_eq!(upload_url.host_str(), Some("127.0.0.1"));
    assert!(upload_url.path().contains("/devstoreaccount1/documents/"));
    assert_eq!(
        upload_url.query_pairs().find(|(name, _)| name == "sp"),
        Some(("sp".into(), "cw".into()))
    );

    stub_handle.abort();
}

#[tokio::test]
async fn bulk_upload_batches_validate_request_shape_and_limits() {
    let (endpoint, _state, stub_handle) = start_stub_server().await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();
    let app = app_with_session(
        &endpoint,
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
    );

    let cases = [
        json!({ "files": [] }),
        json!({
            "files": [{
                "filename": "input.csv",
                "contentType": "text/csv",
                "size": 0,
            }],
        }),
        json!({
            "files": [{
                "filename": "..",
                "contentType": "text/csv",
                "size": 1,
            }],
        }),
        json!({
            "files": (0..101)
                .map(|index| json!({
                    "filename": format!("input-{index}.csv"),
                    "contentType": "text/csv",
                    "size": 1,
                }))
                .collect::<Vec<_>>(),
        }),
        json!({
            "files": [
                {
                    "filename": "part-a.csv",
                    "contentType": "text/csv",
                    "size": 3 * 1024 * 1024 * 1024_u64,
                },
                {
                    "filename": "part-b.csv",
                    "contentType": "text/csv",
                    "size": 3 * 1024 * 1024 * 1024_u64,
                }
            ],
        }),
    ];

    for body in cases {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces/workspace-a/configs/config-v1/uploads/batches")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

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
    assert!(location.ends_with(&format!("/runs/{run_id}")));

    let detail_url = format!("{base_url}{location}");
    let detail = wait_for_run_status(&client, &detail_url, "succeeded").await;
    let output_path = detail["outputPath"].as_str().unwrap();
    let log_path = detail["logPath"].as_str().unwrap();
    assert_eq!(
        output_path,
        format!("workspaces/workspace-a/configs/config-v1/runs/{run_id}/output/normalized.xlsx")
    );
    assert_eq!(
        log_path,
        format!("workspaces/workspace-a/configs/config-v1/runs/{run_id}/logs/events.ndjson")
    );
    assert_eq!(detail["validationIssues"], json!([]));

    let log_bytes = {
        let stub_state = state.lock().unwrap();
        let output_blob = stored_blob(&stub_state, output_path);
        assert_eq!(output_blob.bytes, b"normalized-output");
        let log_blob = stored_blob(&stub_state, log_path);
        let log_bytes = log_blob.bytes.clone();
        let log_text = String::from_utf8(log_bytes.clone()).unwrap();
        assert!(log_text.lines().count() >= 2);
        assert!(log_text.lines().any(|line| {
            let payload = serde_json::from_str::<Value>(line).unwrap();
            payload["event"] == "run.log" && payload["data"]["message"] == "Loaded 12 rows"
        }));
        log_bytes
    };
    let (output_download, downloaded_output_bytes) =
        download_run_artifact(&client, &base_url, run_id, "output").await;
    assert_eq!(output_download["filePath"], output_path);
    assert_eq!(output_download["download"]["method"], "GET");
    assert_eq!(downloaded_output_bytes, b"normalized-output");

    let (log_download, downloaded_log_bytes) =
        download_run_artifact(&client, &base_url, run_id, "log").await;
    assert_eq!(log_download["filePath"], log_path);
    assert_eq!(log_download["download"]["method"], "GET");
    assert_eq!(downloaded_log_bytes, log_bytes);

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn run_downloads_return_conflict_until_artifacts_are_ready() {
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
    let output_pending = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/downloads"
        ))
        .json(&json!({ "artifact": "output" }))
        .send()
        .await
        .unwrap();
    assert_eq!(output_pending.status(), reqwest::StatusCode::CONFLICT);

    let log_pending = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/downloads"
        ))
        .json(&json!({ "artifact": "log" }))
        .send()
        .await
        .unwrap();
    assert_eq!(log_pending.status(), reqwest::StatusCode::CONFLICT);

    let detail_url =
        format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}");
    let detail = wait_for_run_status(&client, &detail_url, "succeeded").await;
    assert!(detail["outputPath"].is_string());
    assert!(detail["logPath"].is_string());
    let events = client
        .get(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/events"
        ))
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(events.status(), reqwest::StatusCode::OK);
    let events_body = events.text().await.unwrap();
    assert!(events_body.contains("event: run.completed"));
    assert!(events_body.contains("id: 1"));

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn cancelling_a_run_marks_it_cancelled_and_emits_final_sse_event() {
    let (endpoint, _state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_run_connector: false,
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

    let detail_url =
        format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}");
    let detail = wait_for_run_status(&client, &detail_url, "cancelled").await;
    assert_eq!(detail["errorMessage"], "Run cancelled.");
    let log_path = detail["logPath"].as_str().unwrap();
    assert!(log_path.ends_with("/logs/events.ndjson"));

    let (download, _log_bytes) = download_run_artifact(&client, &base_url, run_id, "log").await;
    assert_eq!(download["filePath"], log_path);
    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn cancelling_before_connector_ready_stops_the_session_attempt() {
    let (endpoint, _state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        run_connector_delay_ms: 150,
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

    let detail = wait_for_run_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}"),
        "cancelled",
    )
    .await;
    assert_eq!(detail["outputPath"], Value::Null);

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn run_detail_keeps_output_hidden_until_success_and_cancellation_clears_state() {
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

    tokio::time::sleep(Duration::from_millis(50)).await;
    let partial = client
        .get(&detail_url)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(partial["status"], "running");
    assert_eq!(partial["outputPath"], Value::Null);

    let cancel = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}/cancel"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(cancel.status(), reqwest::StatusCode::NO_CONTENT);

    let detail = wait_for_run_status(&client, &detail_url, "cancelled").await;
    assert_eq!(detail["outputPath"], Value::Null);
    assert_eq!(detail["validationIssues"], json!([]));
    assert_eq!(detail["errorMessage"], "Run cancelled.");

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn transient_run_connector_startup_failures_retry_once_and_then_succeed() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        run_connector_disconnect_before_ready_attempts: 1,
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

    let detail = wait_for_run_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{run_id}"),
        "succeeded",
    )
    .await;
    assert!(detail["outputPath"].is_string());

    let state = state.lock().unwrap();
    assert_eq!(state.run_execution_count, 2);

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn queued_runs_stay_pending_until_a_scheduler_slot_is_available() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        run_execution_delay_ms: 200,
        ..PoolStubOptions::default()
    })
    .await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = app_with_session_and_url_and_run_limit(
        &endpoint,
        &format!("http://{address}"),
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
        Some(1),
    );
    let app_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = Client::new();
    let base_url = format!("http://{address}");
    let first_upload = upload_input(
        &client,
        &base_url,
        "first.csv",
        "text/csv",
        b"name\nfirst\n".to_vec(),
    )
    .await;
    let second_upload = upload_input(
        &client,
        &base_url,
        "second.csv",
        "text/csv",
        b"name\nsecond\n".to_vec(),
    )
    .await;

    let first_run = start_run(
        &client,
        &base_url,
        first_upload["filePath"].as_str().unwrap(),
    )
    .await
    .json::<Value>()
    .await
    .unwrap();
    let second_run = start_run(
        &client,
        &base_url,
        second_upload["filePath"].as_str().unwrap(),
    )
    .await
    .json::<Value>()
    .await
    .unwrap();
    let first_run_id = first_run["runId"].as_str().unwrap();
    let second_run_id = second_run["runId"].as_str().unwrap();

    wait_for_run_execution_count(&state, 1).await;

    let second_detail = client
        .get(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{second_run_id}"
        ))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(second_detail["status"], "pending");

    let _ = wait_for_run_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{first_run_id}"),
        "succeeded",
    )
    .await;
    let _ = wait_for_run_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{second_run_id}"),
        "succeeded",
    )
    .await;

    assert_eq!(state.lock().unwrap().run_execution_count, 1);

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn cancelling_a_queued_run_marks_it_cancelled_without_starting_execution() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        run_execution_delay_ms: 200,
        ..PoolStubOptions::default()
    })
    .await;
    let (_tempdir, config_v1, _config_v2, engine) = create_wheels();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = app_with_session_and_url_and_run_limit(
        &endpoint,
        &format!("http://{address}"),
        &[("workspace-a", "config-v1", config_v1.as_path())],
        &engine,
        Some(1),
    );
    let app_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = Client::new();
    let base_url = format!("http://{address}");
    let first_upload = upload_input(
        &client,
        &base_url,
        "first.csv",
        "text/csv",
        b"name\nfirst\n".to_vec(),
    )
    .await;
    let second_upload = upload_input(
        &client,
        &base_url,
        "second.csv",
        "text/csv",
        b"name\nsecond\n".to_vec(),
    )
    .await;

    let first_run = start_run(
        &client,
        &base_url,
        first_upload["filePath"].as_str().unwrap(),
    )
    .await
    .json::<Value>()
    .await
    .unwrap();
    let second_run = start_run(
        &client,
        &base_url,
        second_upload["filePath"].as_str().unwrap(),
    )
    .await
    .json::<Value>()
    .await
    .unwrap();
    let first_run_id = first_run["runId"].as_str().unwrap();
    let second_run_id = second_run["runId"].as_str().unwrap();

    wait_for_run_execution_count(&state, 1).await;

    let cancel = client
        .post(format!(
            "{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{second_run_id}/cancel"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(cancel.status(), reqwest::StatusCode::NO_CONTENT);

    let cancelled = wait_for_run_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{second_run_id}"),
        "cancelled",
    )
    .await;
    assert_eq!(cancelled["outputPath"], Value::Null);
    assert_eq!(cancelled["errorMessage"], "Run cancelled.");

    let _ = wait_for_run_status(
        &client,
        &format!("{base_url}/api/workspaces/workspace-a/configs/config-v1/runs/{first_run_id}"),
        "succeeded",
    )
    .await;

    assert_eq!(state.lock().unwrap().run_execution_count, 1);

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
async fn sandbox_environment_launches_reverse_connect_with_url_and_bearer_token() {
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
    let _ = socket.next().await;

    let execution_codes = state.lock().unwrap().execution_codes.clone();
    assert_eq!(execution_codes.len(), 1);
    assert!(execution_codes[0].contains("reverse-connect"));
    assert!(execution_codes[0].contains(" connect --url "));
    assert!(execution_codes[0].contains("/app/ade/bin/reverse-connect"));
    assert!(extract_connector_url(&execution_codes[0]).is_some());
    assert!(extract_bearer_token(&execution_codes[0]).is_some());

    let uploaded_paths = state
        .lock()
        .unwrap()
        .session_files
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        uploaded_paths
            .iter()
            .any(|path| path.ends_with(":sandbox-environment.tar.gz"))
    );
    assert!(
        uploaded_paths
            .iter()
            .all(|path| !path.contains("ade/configs/"))
    );

    let _ = socket.close(None).await;
    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn terminal_route_streams_terminal_output_over_reverse_connect() {
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

    let _ = socket.close(None).await;
    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn reverse_connect_rendezvous_allows_only_one_connector() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_connector: false,
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
    let (connector_url, bearer_token) = {
        let state = state.lock().unwrap();
        (
            extract_connector_url(&state.execution_codes[0]).unwrap(),
            extract_bearer_token(&state.execution_codes[0]).unwrap(),
        )
    };

    let (mut connector_socket, _response) =
        connect_async(connector_request(&connector_url, &bearer_token))
            .await
            .unwrap();
    connector_socket
        .send(Message::Text(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "connector.hello",
                "params": {
                    "capabilities": ["exec", "pty"],
                    "host": {
                        "arch": "x86_64",
                        "os": "linux"
                    },
                    "version": "test"
                }
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let hello_response = connector_socket
        .next()
        .await
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let hello_response = serde_json::from_str::<Value>(&hello_response).unwrap();
    assert_eq!(hello_response["id"], 1);
    let channel_open = connector_socket
        .next()
        .await
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let channel_open = serde_json::from_str::<Value>(&channel_open).unwrap();
    assert_eq!(channel_open["method"], "channel.open");

    assert!(
        connect_async(connector_request(&connector_url, &bearer_token))
            .await
            .is_err()
    );

    let _ = connector_socket.close(None).await;
    let _ = browser_socket.close(None).await;
    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn browser_disconnect_before_connector_ready_eventually_clears_pending_state() {
    let (endpoint, state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_connector: false,
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
    let connector_url = {
        let state = state.lock().unwrap();
        extract_connector_url(&state.execution_codes[0]).unwrap()
    };

    browser_socket.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(connect_async(&connector_url).await.is_err());

    app_handle.abort();
    stub_handle.abort();
}

#[tokio::test]
async fn reconnect_while_previous_terminal_is_shutting_down_returns_clear_error() {
    let (endpoint, _state, stub_handle) = start_stub_server_with_options(PoolStubOptions {
        auto_connect_terminal_connector: false,
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
