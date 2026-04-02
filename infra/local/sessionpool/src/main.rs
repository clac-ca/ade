use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Write},
    net::TcpStream,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Multipart, Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    io::AsyncWriteExt, net::TcpListener, process::Command, sync::Mutex as AsyncMutex, time::timeout,
};
use tracing::info;
use uuid::Uuid;

const AZURE_SHELL_API_VERSION: &str = "2025-10-02-preview";
const DEFAULT_BEARER_TOKEN: &str = "ade-local-session-token";
const DEFAULT_COOLDOWN_SECONDS: i64 = 3600;
const DEFAULT_EXECUTION_TIMEOUT_SECONDS: u64 = 220;
const DEFAULT_MAX_CONCURRENT_SESSIONS: usize = 5;
const DEFAULT_SHELL: &str = "/bin/bash";

#[derive(Parser)]
#[command(name = "session-pool-emulator")]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    Serve(ServeArgs),
    Healthcheck(HealthcheckArgs),
}

#[derive(Parser)]
struct ServeArgs {
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    #[arg(long, default_value_t = 9000)]
    port: u16,
    #[arg(long, default_value = "/workspace")]
    workspace: PathBuf,
    #[arg(long, default_value = DEFAULT_BEARER_TOKEN)]
    bearer_token: String,
}

#[derive(Parser)]
struct HealthcheckArgs {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 9000)]
    port: u16,
}

#[derive(Clone)]
struct AppState {
    emulator: Arc<SessionPoolEmulator>,
}

struct SessionPoolEmulator {
    app_mount_path: PathBuf,
    baseline: BaselineFixture,
    bearer_token: String,
    config_mount_source_path: PathBuf,
    cooldown_seconds: i64,
    execution_lock: AsyncMutex<()>,
    max_concurrent_sessions: usize,
    mnt_data_mount_path: PathBuf,
    sessions: Mutex<BTreeMap<String, SessionState>>,
    sessions_root: PathBuf,
}

#[derive(Clone, Deserialize)]
struct BaselineFixture {
    #[serde(rename = "apiVersion")]
    api_version: String,
    metadata: Value,
}

#[derive(Clone)]
struct SessionState {
    app_root: PathBuf,
    created_at: OffsetDateTime,
    etag: String,
    executions: BTreeMap<String, ExecutionRecord>,
    expire_at: OffsetDateTime,
    file_content_types: HashMap<String, String>,
    guid: String,
    identifier: String,
    last_accessed_at: OffsetDateTime,
    mnt_data_root: PathBuf,
}

#[derive(Clone)]
struct ExecutionRecord {
    response: ExecutionResponse,
}

#[derive(Debug, Deserialize)]
struct ApiVersionQuery {
    #[serde(rename = "api-version")]
    api_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionQuery {
    #[serde(rename = "api-version")]
    api_version: Option<String>,
    identifier: String,
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    #[serde(rename = "api-version")]
    api_version: Option<String>,
    identifier: String,
    path: Option<String>,
    recursive: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionRequest {
    exec_command_and_args: Option<Vec<String>>,
    output_streams_max_length: Option<usize>,
    shell_command: Option<String>,
    stdin: Option<String>,
    timeout_in_seconds: Option<u64>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionResource {
    created_at: String,
    etag: String,
    expire_at: String,
    identifier: String,
    last_accessed_at: String,
}

#[derive(Debug, Serialize)]
struct SessionListResponse {
    sessions: Vec<SessionResource>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    last_modified_at: String,
    name: String,
    size_in_bytes: u64,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Serialize)]
struct FileListResponse {
    value: Vec<FileRecord>,
}

#[derive(Clone, Debug, Serialize)]
struct ExecutionResponse {
    identifier: String,
    status: String,
    result: ExecutionResult,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionResult {
    execution_time_in_milliseconds: u64,
    stderr: String,
    stdout: String,
}

#[derive(Debug)]
struct ApiError {
    body: Value,
    content_type: &'static str,
    error_code: Option<String>,
    status: StatusCode,
}

impl ApiError {
    fn bad_request(code: &str, message: impl Into<String>) -> Self {
        Self::nested(StatusCode::BAD_REQUEST, code, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::nested(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
            message,
        )
    }

    fn invalid_api_version(version: Option<&str>) -> Self {
        let message = match version {
            Some(version) => format!(
                "The api-version '{version}' is not supported. Supported versions: {}.",
                supported_versions_header()
            ),
            None => format!(
                "The api-version query parameter is required. Supported versions: {}.",
                supported_versions_header()
            ),
        };
        Self::nested(StatusCode::BAD_REQUEST, "InvalidApiVersion", message)
    }

    fn nested(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            body: json!({
                "error": {
                    "code": code,
                    "message": message,
                    "traceId": trace_id(),
                }
            }),
            content_type: "application/json; charset=utf-8",
            error_code: Some(code.to_string()),
            status,
        }
    }

    fn not_found(code: &str, message: impl Into<String>) -> Self {
        Self::nested(StatusCode::NOT_FOUND, code, message)
    }

    fn sessions_count_exceeds_limit(active_sessions: usize, requested_sessions: usize) -> Self {
        Self {
            body: json!({
                "code": "SessionsCountExceedsLimit",
                "message": format!(
                    "The request sessions count is '{requested_sessions}'. There are already '{active_sessions}' active sessions and the maximum active sessions count is '{DEFAULT_MAX_CONCURRENT_SESSIONS}'."
                ),
            }),
            content_type: "application/json; charset=utf-8",
            error_code: Some("SessionsCountExceedsLimit".to_string()),
            status: StatusCode::TOO_MANY_REQUESTS,
        }
    }

    fn unauthorized() -> Self {
        Self::nested(
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "A valid Bearer token is required.",
        )
    }

    fn validation(message: impl Into<String>) -> Self {
        Self {
            body: json!({
                "type": "https://tools.ietf.org/html/rfc9110#section-15.5.1",
                "title": "One or more validation errors occurred.",
                "status": 400,
                "errors": {
                    "$": [message.into()],
                },
                "traceId": trace_id(),
            }),
            content_type: "application/problem+json; charset=utf-8",
            error_code: None,
            status: StatusCode::BAD_REQUEST,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut headers = supported_version_headers();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(self.content_type),
        );
        if let Some(error_code) = self.error_code.as_deref()
            && let Ok(value) = HeaderValue::from_str(error_code)
        {
            headers.insert("x-ms-error-code", value);
        }

        (self.status, headers, Json(self.body)).into_response()
    }
}

impl SessionPoolEmulator {
    fn new(workspace_root: PathBuf, bearer_token: String) -> Result<Self, ApiError> {
        let sessions_root = workspace_root.join("sessions");
        fs::create_dir_all(&sessions_root).map_err(|error| {
            ApiError::internal(format!(
                "Failed to create session root '{}': {error}",
                sessions_root.display()
            ))
        })?;
        fs::create_dir_all("/mnt").map_err(|error| {
            ApiError::internal(format!("Failed to create /mnt for session mounts: {error}"))
        })?;

        let baseline =
            serde_json::from_str::<BaselineFixture>(include_str!("../azure-shell-baseline.json"))
                .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to load the embedded Azure Shell baseline fixture: {error}"
                ))
            })?;
        if baseline.api_version != AZURE_SHELL_API_VERSION {
            return Err(ApiError::internal(format!(
                "Embedded baseline apiVersion '{}' does not match '{}'.",
                baseline.api_version, AZURE_SHELL_API_VERSION
            )));
        }

        Ok(Self {
            app_mount_path: PathBuf::from("/app"),
            baseline,
            bearer_token,
            config_mount_source_path: PathBuf::from("/emulator-configs"),
            cooldown_seconds: DEFAULT_COOLDOWN_SECONDS,
            execution_lock: AsyncMutex::new(()),
            max_concurrent_sessions: DEFAULT_MAX_CONCURRENT_SESSIONS,
            mnt_data_mount_path: PathBuf::from("/mnt/data"),
            sessions: Mutex::new(BTreeMap::new()),
            sessions_root,
        })
    }

    fn delete_file(
        &self,
        identifier: &str,
        directory: Option<&str>,
        filename: &str,
    ) -> Result<(HeaderMap, StatusCode), ApiError> {
        let now = OffsetDateTime::now_utc();
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        self.expire_sessions(&mut sessions, now)?;
        let (new_allocation, session) = self.ensure_session(&mut sessions, identifier, now)?;
        let filename = validate_file_name(filename)?;
        let target = file_target_path(session, directory, &filename)?;
        if !target.is_file() {
            return Err(ApiError::not_found(
                "FileNotFound",
                format!("File '{filename}' was not found."),
            ));
        }
        fs::remove_file(&target).map_err(|error| {
            ApiError::internal(format!(
                "Failed to delete file '{}': {error}",
                target.display()
            ))
        })?;
        session
            .file_content_types
            .remove(&logical_file_key(directory, &filename)?);
        Ok((
            session_headers(session, new_allocation),
            StatusCode::NO_CONTENT,
        ))
    }

    fn delete_session(&self, identifier: &str) -> Result<(), ApiError> {
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        let Some(session) = sessions.remove(identifier) else {
            return Err(ApiError::not_found(
                "NoSessionFoundError",
                format!("Could not find session with name '{identifier}'."),
            ));
        };
        if session.mnt_data_root.exists() {
            fs::remove_dir_all(&session.mnt_data_root).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to remove session data '{}': {error}",
                    session.mnt_data_root.display()
                ))
            })?;
        }
        if session.app_root.exists() {
            fs::remove_dir_all(&session.app_root).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to remove session uploads '{}': {error}",
                    session.app_root.display()
                ))
            })?;
        }
        Ok(())
    }

    fn download_file(
        &self,
        identifier: &str,
        directory: Option<&str>,
        filename: &str,
    ) -> Result<(HeaderMap, Bytes), ApiError> {
        let now = OffsetDateTime::now_utc();
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        self.expire_sessions(&mut sessions, now)?;
        let (new_allocation, session) = self.ensure_session(&mut sessions, identifier, now)?;
        let filename = validate_file_name(filename)?;
        let target = file_target_path(session, directory, &filename)?;
        if !target.is_file() {
            return Err(ApiError::not_found(
                "FileNotFound",
                format!("File '{filename}' was not found."),
            ));
        }
        let content = fs::read(&target).map_err(|error| {
            ApiError::internal(format!(
                "Failed to read session file '{}': {error}",
                target.display()
            ))
        })?;
        let mut headers = session_headers(session, new_allocation);
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&file_content_type(session, directory, &filename, true)?)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        );
        Ok((headers, Bytes::from(content)))
    }

    async fn execute(
        &self,
        identifier: &str,
        request: ExecutionRequest,
    ) -> Result<(HeaderMap, ExecutionResponse), ApiError> {
        let now = OffsetDateTime::now_utc();
        let (new_allocation, session_guid, app_root, mnt_data_root, execution_id) = {
            let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
            self.expire_sessions(&mut sessions, now)?;
            let (new_allocation, session) = self.ensure_session(&mut sessions, identifier, now)?;
            (
                new_allocation,
                session.guid.clone(),
                session.app_root.clone(),
                session.mnt_data_root.clone(),
                Uuid::new_v4().simple().to_string(),
            )
        };

        let execution = self
            .run_execution(identifier, request, &app_root, &mnt_data_root)
            .await?;

        {
            let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
            if let Some(session) = sessions.get_mut(identifier) {
                self.touch_session(session, OffsetDateTime::now_utc());
                session.executions.insert(
                    execution_id.clone(),
                    ExecutionRecord {
                        response: execution.clone(),
                    },
                );
            }
        }

        let mut headers = supported_version_headers();
        headers.insert(
            "x-ms-session-guid",
            HeaderValue::from_str(&session_guid)
                .unwrap_or_else(|_| HeaderValue::from_static("invalid-guid")),
        );
        headers.insert(
            "x-ms-new-allocation",
            HeaderValue::from_static(if new_allocation { "true" } else { "false" }),
        );
        headers.insert(
            "operation-id",
            HeaderValue::from_str(&execution_id)
                .unwrap_or_else(|_| HeaderValue::from_static("invalid-operation-id")),
        );
        headers.insert(
            "Operation-Location",
            HeaderValue::from_str(&format!("/executions/{execution_id}"))
                .unwrap_or_else(|_| HeaderValue::from_static("/executions/invalid-operation-id")),
        );

        Ok((headers, execution))
    }

    fn execution(
        &self,
        identifier: &str,
        execution_id: &str,
    ) -> Result<(HeaderMap, ExecutionResponse), ApiError> {
        let now = OffsetDateTime::now_utc();
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        self.expire_sessions(&mut sessions, now)?;
        let Some(session) = sessions.get_mut(identifier) else {
            return Err(ApiError::not_found(
                "NoSessionFoundError",
                format!("Could not find session with name '{identifier}'."),
            ));
        };
        self.touch_session(session, now);
        let Some(execution) = session.executions.get(execution_id) else {
            return Err(ApiError::not_found(
                "ExecutionNotFound",
                format!("Execution '{execution_id}' was not found."),
            ));
        };
        let mut headers = supported_version_headers();
        headers.insert(
            "x-ms-session-guid",
            HeaderValue::from_str(&session.guid)
                .unwrap_or_else(|_| HeaderValue::from_static("invalid-guid")),
        );
        Ok((headers, execution.response.clone()))
    }

    fn expire_sessions(
        &self,
        sessions: &mut BTreeMap<String, SessionState>,
        now: OffsetDateTime,
    ) -> Result<(), ApiError> {
        let expired = sessions
            .iter()
            .filter(|(_, session)| session.expire_at <= now)
            .map(|(identifier, session)| {
                (
                    identifier.clone(),
                    session.mnt_data_root.clone(),
                    session.app_root.clone(),
                )
            })
            .collect::<Vec<_>>();
        for (identifier, _, _) in &expired {
            sessions.remove(identifier);
        }
        for (_, mnt_data_root, app_root) in expired {
            if mnt_data_root.exists() {
                fs::remove_dir_all(&mnt_data_root).map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to remove expired session data '{}': {error}",
                        mnt_data_root.display()
                    ))
                })?;
            }
            if app_root.exists() {
                fs::remove_dir_all(&app_root).map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to remove expired session uploads '{}': {error}",
                        app_root.display()
                    ))
                })?;
            }
        }
        Ok(())
    }

    fn file(
        &self,
        identifier: &str,
        directory: Option<&str>,
        filename: &str,
    ) -> Result<(HeaderMap, FileRecord), ApiError> {
        let now = OffsetDateTime::now_utc();
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        self.expire_sessions(&mut sessions, now)?;
        let (new_allocation, session) = self.ensure_session(&mut sessions, identifier, now)?;
        let filename = validate_file_name(filename)?;
        let target = file_target_path(session, directory, &filename)?;
        if !target.exists() {
            return Err(ApiError::not_found(
                "FileNotFound",
                format!("File '{filename}' was not found."),
            ));
        }

        Ok((
            session_headers(session, new_allocation),
            file_record(session, &target, directory, false)?,
        ))
    }

    fn list_files(
        &self,
        identifier: &str,
        directory: Option<&str>,
        recursive: bool,
    ) -> Result<(HeaderMap, FileListResponse), ApiError> {
        let now = OffsetDateTime::now_utc();
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        self.expire_sessions(&mut sessions, now)?;
        let (new_allocation, session) = self.ensure_session(&mut sessions, identifier, now)?;
        let target = list_target_path(session, directory)?;

        if !target.exists() {
            return Ok((
                session_headers(session, new_allocation),
                FileListResponse { value: Vec::new() },
            ));
        }
        if !target.is_dir() {
            return Err(ApiError::bad_request(
                "FilePathInvalid",
                "The path must refer to a directory.",
            ));
        }

        let mut files = Vec::new();
        if recursive {
            walk_directory(session, directory, &target, &mut files)?;
        } else {
            for entry in fs::read_dir(&target).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read session files under '{}': {error}",
                    target.display()
                ))
            })? {
                let entry = entry.map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to read a session file entry under '{}': {error}",
                        target.display()
                    ))
                })?;
                files.push(file_record(session, &entry.path(), directory, false)?);
            }
        }
        files.sort_by(|left, right| left.name.cmp(&right.name));

        Ok((
            session_headers(session, new_allocation),
            FileListResponse { value: files },
        ))
    }

    fn list_sessions(&self) -> Result<SessionListResponse, ApiError> {
        let now = OffsetDateTime::now_utc();
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        self.expire_sessions(&mut sessions, now)?;
        let mut resources = sessions
            .values()
            .map(SessionResource::from)
            .collect::<Vec<_>>();
        resources.sort_by(|left, right| left.identifier.cmp(&right.identifier));
        Ok(SessionListResponse {
            sessions: resources,
        })
    }

    fn metadata(&self) -> Value {
        self.baseline.metadata.clone()
    }

    fn point_mount(&self, mount_path: &Path, target: &Path) -> Result<(), ApiError> {
        if let Ok(metadata) = fs::symlink_metadata(mount_path) {
            if metadata.file_type().is_symlink() || metadata.is_file() {
                fs::remove_file(mount_path).map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to replace mount '{}': {error}",
                        mount_path.display()
                    ))
                })?;
            } else if metadata.is_dir() {
                fs::remove_dir_all(mount_path).map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to replace mount '{}': {error}",
                        mount_path.display()
                    ))
                })?;
            }
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, mount_path).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to point '{}' at '{}': {error}",
                    mount_path.display(),
                    target.display()
                ))
            })?;
        }

        Ok(())
    }

    async fn run_execution(
        &self,
        identifier: &str,
        request: ExecutionRequest,
        app_root: &Path,
        mnt_data_root: &Path,
    ) -> Result<ExecutionResponse, ApiError> {
        let (mut command, timeout_seconds, output_limit, stdin) = build_command(request)?;
        let started_at = Instant::now();
        let _guard = self.execution_lock.lock().await;
        self.point_mount(&self.app_mount_path, app_root)?;
        self.point_mount(&self.mnt_data_mount_path, mnt_data_root)?;

        command.current_dir(&self.mnt_data_mount_path);
        command.env("HOME", "/root");
        command.env("PWD", "/mnt/data");
        command.env("SHELL", DEFAULT_SHELL);
        command.kill_on_drop(true);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        let mut child = command.spawn().map_err(|error| {
            ApiError::internal(format!(
                "Failed to start shell command for session '{identifier}': {error}"
            ))
        })?;

        if let Some(stdin) = stdin
            && let Some(mut child_stdin) = child.stdin.take()
        {
            child_stdin
                .write_all(stdin.as_bytes())
                .await
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to write stdin for session '{identifier}': {error}"
                    ))
                })?;
        }

        let output = match timeout(
            Duration::from_secs(timeout_seconds),
            child.wait_with_output(),
        )
        .await
        {
            Ok(output) => output.map_err(|error| {
                ApiError::internal(format!(
                    "Failed to collect shell output for session '{identifier}': {error}"
                ))
            })?,
            Err(_) => {
                return Ok(ExecutionResponse {
                    identifier: identifier.to_string(),
                    status: "-1".to_string(),
                    result: ExecutionResult {
                        execution_time_in_milliseconds: timeout_seconds.saturating_mul(1000),
                        stderr: String::new(),
                        stdout: String::new(),
                    },
                });
            }
        };

        let stdout = truncate_output(output.stdout, output_limit);
        let stderr = truncate_output(output.stderr, output_limit);

        Ok(ExecutionResponse {
            identifier: identifier.to_string(),
            status: if output.status.success() {
                "0".to_string()
            } else {
                "1".to_string()
            },
            result: ExecutionResult {
                execution_time_in_milliseconds: started_at.elapsed().as_millis() as u64,
                stderr,
                stdout,
            },
        })
    }

    fn session(&self, identifier: &str) -> Result<(HeaderMap, SessionResource), ApiError> {
        let now = OffsetDateTime::now_utc();
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        self.expire_sessions(&mut sessions, now)?;
        let Some(session) = sessions.get_mut(identifier) else {
            return Err(ApiError::not_found(
                "NoSessionFoundError",
                format!("Could not find session with name '{identifier}'."),
            ));
        };
        self.touch_session(session, now);
        let mut headers = supported_version_headers();
        headers.insert(
            "x-ms-session-etag",
            HeaderValue::from_str(&session.etag)
                .unwrap_or_else(|_| HeaderValue::from_static("invalid-etag")),
        );
        Ok((headers, SessionResource::from(&*session)))
    }

    fn touch_session(&self, session: &mut SessionState, now: OffsetDateTime) {
        session.last_accessed_at = now;
        session.expire_at = now + time::Duration::seconds(self.cooldown_seconds);
    }

    fn upload_file(
        &self,
        identifier: &str,
        directory: Option<&str>,
        filename: &str,
        content_type: Option<&str>,
        content: &[u8],
    ) -> Result<(HeaderMap, FileRecord), ApiError> {
        let now = OffsetDateTime::now_utc();
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        self.expire_sessions(&mut sessions, now)?;
        let (new_allocation, session) = self.ensure_session(&mut sessions, identifier, now)?;
        let filename = validate_file_name(filename)?;
        let target = file_target_path(session, directory, &filename)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create upload directory '{}': {error}",
                    parent.display()
                ))
            })?;
        }
        fs::write(&target, content).map_err(|error| {
            ApiError::internal(format!(
                "Failed to write uploaded file '{}': {error}",
                target.display()
            ))
        })?;

        let content_type = content_type
            .map(normalized_uploaded_content_type)
            .filter(|content_type| !content_type.is_empty())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        session
            .file_content_types
            .insert(logical_file_key(directory, &filename)?, content_type);

        Ok((
            session_headers(session, new_allocation),
            file_record(session, &target, directory, false)?,
        ))
    }

    fn ensure_session<'a>(
        &self,
        sessions: &'a mut BTreeMap<String, SessionState>,
        identifier: &str,
        now: OffsetDateTime,
    ) -> Result<(bool, &'a mut SessionState), ApiError> {
        let new_allocation = if sessions.contains_key(identifier) {
            false
        } else {
            if sessions.len() >= self.max_concurrent_sessions {
                return Err(ApiError::sessions_count_exceeds_limit(sessions.len(), 1));
            }
            let root = self.sessions_root.join(identifier);
            let mnt_data_root = root.join("mnt-data");
            let app_root = root.join("app");
            fs::create_dir_all(&mnt_data_root).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create session data directory '{}': {error}",
                    mnt_data_root.display()
                ))
            })?;
            fs::create_dir_all(&app_root).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create session app directory '{}': {error}",
                    app_root.display()
                ))
            })?;
            fs::set_permissions(&mnt_data_root, fs::Permissions::from_mode(0o777)).map_err(
                |error| {
                    ApiError::internal(format!(
                        "Failed to set permissions on '{}': {error}",
                        mnt_data_root.display()
                    ))
                },
            )?;
            fs::set_permissions(&app_root, fs::Permissions::from_mode(0o755)).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to set permissions on '{}': {error}",
                    app_root.display()
                ))
            })?;
            let config_mount_path = mnt_data_root.join("ade/configs");
            let config_mount_parent = config_mount_path.parent().ok_or_else(|| {
                ApiError::internal(format!(
                    "Failed to derive config mount parent from '{}'.",
                    config_mount_path.display()
                ))
            })?;
            fs::create_dir_all(config_mount_parent).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create config mount parent '{}': {error}",
                    config_mount_parent.display()
                ))
            })?;
            if self.config_mount_source_path.exists() {
                copy_directory_contents(&self.config_mount_source_path, &config_mount_path)?;
            } else {
                fs::create_dir_all(&config_mount_path).map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to create config mount directory '{}': {error}",
                        config_mount_path.display()
                    ))
                })?;
            }
            sessions.insert(
                identifier.to_string(),
                SessionState {
                    app_root,
                    created_at: now,
                    etag: format!("0--{}", Uuid::new_v4()),
                    executions: BTreeMap::new(),
                    expire_at: now + time::Duration::seconds(self.cooldown_seconds),
                    file_content_types: HashMap::new(),
                    guid: Uuid::new_v4().to_string(),
                    identifier: identifier.to_string(),
                    last_accessed_at: now,
                    mnt_data_root,
                },
            );
            true
        };
        let session = sessions
            .get_mut(identifier)
            .expect("session should exist after ensure");
        self.touch_session(session, now);
        Ok((new_allocation, session))
    }
}

impl From<&SessionState> for SessionResource {
    fn from(session: &SessionState) -> Self {
        Self {
            created_at: format_rfc3339(session.created_at),
            etag: session.etag.clone(),
            expire_at: format_rfc3339(session.expire_at),
            identifier: session.identifier.clone(),
            last_accessed_at: format_rfc3339(session.last_accessed_at),
        }
    }
}

async fn delete_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(filename): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    let (response_headers, status) =
        state
            .emulator
            .delete_file(&query.identifier, query.path.as_deref(), &filename)?;
    Ok((status, response_headers).into_response())
}

async fn delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SessionQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    state.emulator.delete_session(&query.identifier)?;
    Ok((StatusCode::NO_CONTENT, supported_version_headers()).into_response())
}

async fn execute(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SessionQuery>,
    Json(body): Json<ExecutionRequest>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    let (response_headers, payload) = state.emulator.execute(&query.identifier, body).await?;
    Ok((response_headers, Json(payload)).into_response())
}

async fn execution(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(execution_id): AxumPath<String>,
    Query(query): Query<SessionQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    let (response_headers, payload) = state.emulator.execution(&query.identifier, &execution_id)?;
    Ok((response_headers, Json(payload)).into_response())
}

async fn file(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(filename): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    let (response_headers, payload) =
        state
            .emulator
            .file(&query.identifier, query.path.as_deref(), &filename)?;
    Ok((response_headers, Json(payload)).into_response())
}

async fn file_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(filename): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    let (response_headers, content) =
        state
            .emulator
            .download_file(&query.identifier, query.path.as_deref(), &filename)?;
    Ok((response_headers, content).into_response())
}

fn file_content_type(
    session: &SessionState,
    directory: Option<&str>,
    filename: &str,
    is_file: bool,
) -> Result<String, ApiError> {
    if !is_file {
        return Ok("application/octet-stream".to_string());
    }
    Ok(session
        .file_content_types
        .get(&logical_file_key(directory, filename)?)
        .cloned()
        .unwrap_or_else(|| "application/octet-stream".to_string()))
}

fn file_record(
    session: &SessionState,
    path: &Path,
    directory: Option<&str>,
    recursive: bool,
) -> Result<FileRecord, ApiError> {
    let metadata = fs::metadata(path).map_err(|error| {
        ApiError::internal(format!(
            "Failed to read file metadata '{}': {error}",
            path.display()
        ))
    })?;
    let modified_at = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let modified_at = OffsetDateTime::from(modified_at)
        .format(&Rfc3339)
        .unwrap_or_else(|_| format_rfc3339(OffsetDateTime::UNIX_EPOCH));
    let name = if recursive {
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string()
    };
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    Ok(FileRecord {
        content_type: if metadata.is_file() {
            Some(file_content_type(session, directory, filename, true)?)
        } else {
            None
        },
        last_modified_at: modified_at,
        name,
        size_in_bytes: metadata.len(),
        kind: if metadata.is_dir() {
            "directory"
        } else {
            "file"
        },
    })
}

fn file_target_path(
    session: &SessionState,
    directory: Option<&str>,
    filename: &str,
) -> Result<PathBuf, ApiError> {
    let filename = validate_file_name(filename)?;
    let parent = list_target_path(session, directory)?;
    Ok(parent.join(filename))
}

fn format_rfc3339(value: OffsetDateTime) -> String {
    value
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn list_files(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FileQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    let (response_headers, payload) = state.emulator.list_files(
        &query.identifier,
        query.path.as_deref(),
        query.recursive.unwrap_or(false),
    )?;
    Ok((response_headers, Json(payload)).into_response())
}

async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ApiVersionQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    Ok((
        supported_version_headers(),
        Json(state.emulator.list_sessions()?),
    )
        .into_response())
}

fn list_target_path(session: &SessionState, directory: Option<&str>) -> Result<PathBuf, ApiError> {
    match normalized_directory(directory)? {
        Some(directory) => Ok(resolve_session_directory(session, &directory)),
        None => Ok(session.mnt_data_root.clone()),
    }
}

fn resolve_session_directory(session: &SessionState, directory: &str) -> PathBuf {
    if directory == "mnt/data" {
        return session.mnt_data_root.clone();
    }
    if let Some(relative) = directory.strip_prefix("mnt/data/") {
        return session.mnt_data_root.join(relative);
    }
    if directory == "app" {
        return session.app_root.clone();
    }
    if let Some(relative) = directory.strip_prefix("app/") {
        return session.app_root.join(relative);
    }
    session.app_root.join(directory)
}

fn logical_file_key(directory: Option<&str>, filename: &str) -> Result<String, ApiError> {
    let filename = validate_file_name(filename)?;
    Ok(match normalized_directory(directory)? {
        Some(directory) => format!("{directory}/{filename}"),
        None => filename,
    })
}

fn normalize_directory_path(path: &str) -> Result<String, ApiError> {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Err(ApiError::bad_request(
            "FilePathInvalid",
            "A file path is required.",
        ));
    }
    Ok(trimmed.to_string())
}

fn normalized_directory(path: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(path) = path
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != ".")
    else {
        return Ok(None);
    };
    let normalized = normalize_directory_path(path)?;
    validate_directory_path(&normalized)?;
    Ok(Some(normalized))
}

async fn metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ApiVersionQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    Ok((supported_version_headers(), Json(state.emulator.metadata())).into_response())
}

fn supported_version_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "api-supported-versions",
        HeaderValue::from_str(supported_versions_header())
            .unwrap_or_else(|_| HeaderValue::from_static(AZURE_SHELL_API_VERSION)),
    );
    headers
}

fn supported_versions_header() -> &'static str {
    option_env!("ADE_SESSION_POOL_SUPPORTED_VERSIONS")
        .unwrap_or("2025-02-02-preview, 2025-10-02-preview")
}

fn normalized_uploaded_content_type(content_type: &str) -> String {
    let content_type = content_type.trim();
    if content_type.starts_with("text/") && !content_type.contains("charset=") {
        format!("{content_type}; charset=utf-8")
    } else {
        content_type.to_string()
    }
}

fn session_headers(session: &SessionState, new_allocation: bool) -> HeaderMap {
    let mut headers = supported_version_headers();
    headers.insert(
        "x-ms-session-guid",
        HeaderValue::from_str(&session.guid)
            .unwrap_or_else(|_| HeaderValue::from_static("invalid-guid")),
    );
    headers.insert(
        "x-ms-new-allocation",
        HeaderValue::from_static(if new_allocation { "true" } else { "false" }),
    );
    headers
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/metadata", get(metadata))
        .route("/listSessions", get(list_sessions))
        .route("/session", get(session).delete(delete_session))
        .route("/executions", post(execute))
        .route("/executions/{execution_id}", get(execution))
        .route("/files", get(list_files).post(upload_file))
        .route("/files/{filename}/content", get(file_content))
        .route("/files/{filename}", get(file).delete(delete_file))
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}

fn authorize(headers: &HeaderMap, emulator: &SessionPoolEmulator) -> Result<(), ApiError> {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(ApiError::unauthorized());
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(ApiError::unauthorized());
    };
    if token.trim() != emulator.bearer_token {
        return Err(ApiError::unauthorized());
    }
    Ok(())
}

fn build_command(
    request: ExecutionRequest,
) -> Result<(Command, u64, Option<usize>, Option<String>), ApiError> {
    let timeout_seconds = request
        .timeout_in_seconds
        .unwrap_or(DEFAULT_EXECUTION_TIMEOUT_SECONDS);
    match (request.shell_command, request.exec_command_and_args) {
        (Some(shell_command), None) => {
            if shell_command.trim().is_empty() {
                return Err(ApiError::validation("shellCommand is required."));
            }
            let mut command = Command::new(DEFAULT_SHELL);
            command.arg("-lc").arg(shell_command);
            Ok((
                command,
                timeout_seconds,
                request.output_streams_max_length,
                request.stdin,
            ))
        }
        (None, Some(command_and_args)) => {
            if command_and_args.is_empty() {
                return Err(ApiError::validation(
                    "execCommandAndArgs must not be empty.",
                ));
            }
            let mut command = Command::new(&command_and_args[0]);
            command.args(&command_and_args[1..]);
            Ok((
                command,
                timeout_seconds,
                request.output_streams_max_length,
                request.stdin,
            ))
        }
        (Some(_), Some(_)) => Err(ApiError::validation(
            "Specify either shellCommand or execCommandAndArgs, but not both.",
        )),
        (None, None) => Err(ApiError::validation(
            "Either shellCommand or execCommandAndArgs is required.",
        )),
    }
}

fn trace_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn truncate_output(bytes: Vec<u8>, limit: Option<usize>) -> String {
    match limit {
        Some(limit) if bytes.len() > limit => String::from_utf8_lossy(&bytes[..limit]).into_owned(),
        _ => String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SessionQuery>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    let (response_headers, payload) = state.emulator.session(&query.identifier)?;
    Ok((response_headers, Json(payload)).into_response())
}

async fn upload_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FileQuery>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    authorize(&headers, &state.emulator)?;
    validate_api_version(query.api_version.as_deref())?;
    let field = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::validation(format!("Failed to read multipart body: {error}")))?
        .ok_or_else(|| ApiError::validation("Multipart body must include a file field."))?;
    let filename = field
        .file_name()
        .ok_or_else(|| ApiError::validation("Uploaded file must include a filename."))?
        .to_string();
    let content_type = field.content_type().map(ToOwned::to_owned);
    let bytes = field.bytes().await.map_err(|error| {
        ApiError::validation(format!("Failed to read uploaded file bytes: {error}"))
    })?;

    let (response_headers, payload) = state.emulator.upload_file(
        &query.identifier,
        query.path.as_deref(),
        &filename,
        content_type.as_deref(),
        &bytes,
    )?;
    Ok((response_headers, Json(payload)).into_response())
}

fn validate_api_version(version: Option<&str>) -> Result<(), ApiError> {
    if version == Some(AZURE_SHELL_API_VERSION) {
        Ok(())
    } else {
        Err(ApiError::invalid_api_version(version))
    }
}

fn validate_directory_path(path: &str) -> Result<(), ApiError> {
    for segment in path.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains('.')
            || segment
                .chars()
                .any(|character| !is_allowed_path_character(character))
        {
            return Err(ApiError::bad_request(
                "FilePathInvalid",
                format!(
                    "File Path '/{path}' is invalid because 'path cannot contain any reserved file path characters'."
                ),
            ));
        }
    }
    Ok(())
}

fn validate_file_name(filename: &str) -> Result<String, ApiError> {
    let trimmed = filename.trim().trim_matches('/');
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed
            .chars()
            .any(|character| !is_allowed_file_character(character))
    {
        return Err(ApiError::bad_request(
            "FileNameInvalid",
            format!(
                "File Name '{trimmed}' is invalid because 'filename cannot contain any reserved file path characters'."
            ),
        ));
    }
    Ok(trimmed.to_string())
}

fn is_allowed_file_character(character: char) -> bool {
    character.is_alphanumeric()
        || character == ' '
        || matches!(
            character,
            '-' | '_' | '.' | '@' | '$' | '&' | '=' | ';' | ',' | '#' | '%' | '^' | '(' | ')'
        )
}

fn is_allowed_path_character(character: char) -> bool {
    is_allowed_file_character(character) && character != '.'
}

fn walk_directory(
    session: &SessionState,
    directory: Option<&str>,
    root: &Path,
    files: &mut Vec<FileRecord>,
) -> Result<(), ApiError> {
    for entry in fs::read_dir(root).map_err(|error| {
        ApiError::internal(format!(
            "Failed to read session files under '{}': {error}",
            root.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            ApiError::internal(format!(
                "Failed to read a session file entry under '{}': {error}",
                root.display()
            ))
        })?;
        let path = entry.path();
        files.push(file_record(session, &path, directory, true)?);
        if path.is_dir() {
            walk_directory(session, directory, &path, files)?;
        }
    }
    Ok(())
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<(), ApiError> {
    fs::create_dir_all(destination).map_err(|error| {
        ApiError::internal(format!(
            "Failed to create directory '{}': {error}",
            destination.display()
        ))
    })?;

    for entry in fs::read_dir(source).map_err(|error| {
        ApiError::internal(format!(
            "Failed to read directory '{}': {error}",
            source.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            ApiError::internal(format!(
                "Failed to read a directory entry under '{}': {error}",
                source.display()
            ))
        })?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to inspect '{}': {error}",
                    source_path.display()
                ))
            })?
            .is_dir()
        {
            copy_directory_contents(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to copy '{}' to '{}': {error}",
                    source_path.display(),
                    destination_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .init();

    let cli = Cli::parse();
    match cli.command {
        CliCommand::Serve(args) => serve(args).await,
        CliCommand::Healthcheck(args) => healthcheck(args),
    }
}

async fn serve(args: ServeArgs) {
    let emulator = Arc::new(
        SessionPoolEmulator::new(args.workspace, args.bearer_token)
            .expect("session-pool emulator should start"),
    );
    let listener = TcpListener::bind((args.host.as_str(), args.port))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "Failed to bind session pool emulator on {}:{}: {error}",
                args.host, args.port
            )
        });
    info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app(AppState { emulator }))
        .await
        .expect("session-pool emulator server should stay up");
}

fn healthcheck(args: HealthcheckArgs) {
    let mut stream = TcpStream::connect((args.host.as_str(), args.port)).unwrap_or_else(|error| {
        panic!("Failed to connect to session pool emulator health endpoint: {error}")
    });
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap_or_else(|error| {
            panic!("Failed to write the session pool health check request: {error}")
        });
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .unwrap_or_else(|error| {
            panic!("Failed to read the session pool health check response: {error}")
        });
    assert!(
        response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200"),
        "Unexpected health response: {response}"
    );
}
