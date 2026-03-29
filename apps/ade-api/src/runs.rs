use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::ws::{Message, WebSocket},
    http::StatusCode,
};
use hmac::{Hmac, Mac};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::{
    sync::{broadcast, oneshot, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    artifacts::{
        ArtifactAccessGrant, ArtifactStoreHandle, artifact_store_from_env, output_path_for_run,
        resolve_access_url, upload_id, upload_path_for_file, validate_input_path,
        verify_local_artifact_access,
    },
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    run_store::{
        RunEvent, RunEventPayload, RunPhase, RunStatus, RunStore, RunStoreHandle, StoredRun,
    },
    session::{CreateRunRequest, RunValidationIssue, Scope, SessionOperationMetadata, SessionRuntimeArtifacts, SessionService},
    unix_time_ms,
};

const APP_URL_ENV_NAME: &str = "ADE_APP_URL";
const BRIDGE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const BRIDGE_TOKEN_TTL_MS: u64 = 60_000;
const RUN_ACCESS_TTL_SECONDS: u64 = 900;
const RUN_EXECUTION_TIMEOUT_SECONDS: u64 = 900;
const RUN_MAX_ATTEMPTS: i32 = 2;

const BOOTSTRAP_TEMPLATE: &str = include_str!("runs/bootstrap.py.tmpl");

#[derive(Clone)]
pub struct RunService {
    active_runs: ActiveRunManager,
    app_url: Url,
    artifact_store: ArtifactStoreHandle,
    bridge_manager: PendingRunBridgeManager,
    run_store: RunStoreHandle,
    session_secret: String,
    session_service: Arc<SessionService>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateUploadRequest {
    pub content_type: Option<String>,
    pub filename: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UploadInstruction {
    pub expires_at: String,
    pub headers: HashMap<String, String>,
    pub method: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateUploadResponse {
    pub file_path: String,
    pub upload: UploadInstruction,
    pub upload_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AsyncRunResponse {
    pub events_url: String,
    pub input_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    pub run_id: String,
    pub status: RunStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDetailResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub input_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<RunPhase>,
    pub run_id: String,
    pub status: RunStatus,
    pub validation_issues: Vec<RunValidationIssue>,
}

pub(crate) struct RunEventFeed {
    pub(crate) replay: Vec<RunEvent>,
    pub(crate) subscription: Option<broadcast::Receiver<RunEvent>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunCreatedEvent {
    pub run_id: String,
    pub status: RunStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunStatusEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub phase: RunPhase,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_guid: Option<String>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timings: Option<crate::run_store::RunTimings>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunLogEvent {
    pub level: String,
    pub message: String,
    pub phase: RunPhase,
    pub run_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunErrorEvent {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<RunPhase>,
    pub retriable: bool,
    pub run_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunResultEvent {
    pub output_path: String,
    pub run_id: String,
    pub validation_issues: Vec<RunValidationIssue>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunCompletedEvent {
    pub final_status: RunStatus,
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum RunBridgeClientMessage {
    Error {
        #[serde(default)]
        phase: Option<RunPhase>,
        message: String,
        retriable: bool,
    },
    Log {
        level: String,
        message: String,
        phase: RunPhase,
    },
    Ready,
    Result {
        #[serde(rename = "outputPath")]
        output_path: String,
        #[serde(rename = "validationIssues")]
        validation_issues: Vec<RunValidationIssue>,
    },
    Status {
        phase: RunPhase,
        state: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum RunBridgeServerMessage {
    Cancel,
}

#[derive(Clone, Debug)]
struct AttemptSuccess {
    output_path: String,
    validation_issues: Vec<RunValidationIssue>,
}

struct RunAttempt<'a> {
    attempt: i32,
    input_path: &'a str,
    output_path: &'a str,
    run_id: &'a str,
    scope: &'a Scope,
    timeout_in_seconds: Option<u64>,
}

#[derive(Debug)]
struct AttemptFailure {
    emitted_error: bool,
    error: AppError,
    phase: Option<RunPhase>,
    retriable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapArtifactAccess {
    headers: HashMap<String, String>,
    method: String,
    url: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunBootstrapConfig {
    config_package_name: &'static str,
    config_version: String,
    config_wheel_path: String,
    bridge_url: String,
    engine_package_name: &'static str,
    engine_version: String,
    engine_wheel_path: String,
    input_download: BootstrapArtifactAccess,
    install_lock_path: String,
    local_input_path: String,
    local_output_dir: String,
    output_path: String,
    output_upload: BootstrapArtifactAccess,
}

#[derive(Clone)]
struct ActiveRunState {
    broadcaster: broadcast::Sender<RunEvent>,
    cancel_tx: watch::Sender<bool>,
}

#[derive(Clone, Default)]
struct ActiveRunManager {
    inner: Arc<Mutex<HashMap<String, ActiveRunState>>>,
}

impl ActiveRunManager {
    fn register(&self, run_id: &str) -> ActiveRunHandle {
        let (broadcaster, _) = broadcast::channel(256);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let state = ActiveRunState {
            broadcaster: broadcaster.clone(),
            cancel_tx,
        };

        self.inner
            .lock()
            .expect("active run lock poisoned")
            .insert(run_id.to_string(), state);

        ActiveRunHandle {
            cancel_rx,
            manager: self.clone(),
            run_id: run_id.to_string(),
            sender: broadcaster,
        }
    }

    fn cancel(&self, run_id: &str) -> bool {
        let Some(state) = self
            .inner
            .lock()
            .expect("active run lock poisoned")
            .get(run_id)
            .cloned()
        else {
            return false;
        };

        let _ = state.cancel_tx.send(true);
        true
    }

    fn remove(&self, run_id: &str) {
        self.inner
            .lock()
            .expect("active run lock poisoned")
            .remove(run_id);
    }

    fn subscribe(&self, run_id: &str) -> Option<broadcast::Receiver<RunEvent>> {
        self.inner
            .lock()
            .expect("active run lock poisoned")
            .get(run_id)
            .map(|state| state.broadcaster.subscribe())
    }
}

struct ActiveRunHandle {
    cancel_rx: watch::Receiver<bool>,
    manager: ActiveRunManager,
    run_id: String,
    sender: broadcast::Sender<RunEvent>,
}

impl ActiveRunHandle {
    fn is_cancelled(&self) -> bool {
        *self.cancel_rx.borrow()
    }

    fn sender(&self) -> &broadcast::Sender<RunEvent> {
        &self.sender
    }
}

impl Drop for ActiveRunHandle {
    fn drop(&mut self) {
        self.manager.remove(&self.run_id);
    }
}

#[derive(Clone, Default)]
struct PendingRunBridgeManager {
    inner: Arc<Mutex<HashMap<String, PendingRunBridgeEntry>>>,
}

impl PendingRunBridgeManager {
    fn cancel(&self, bridge_id: &str) {
        self.inner
            .lock()
            .expect("pending run bridge lock poisoned")
            .remove(bridge_id);
    }

    fn claim(&self, bridge_id: &str) -> Result<oneshot::Sender<WebSocket>, AppError> {
        self.inner
            .lock()
            .expect("pending run bridge lock poisoned")
            .remove(bridge_id)
            .map(|entry| entry.bridge_tx)
            .ok_or_else(|| AppError::not_found("Run bridge not found."))
    }

    fn create(&self) -> PendingRunBridge {
        let bridge_id = Uuid::new_v4().simple().to_string();
        let (bridge_tx, bridge_rx) = oneshot::channel();

        self.inner
            .lock()
            .expect("pending run bridge lock poisoned")
            .insert(bridge_id.clone(), PendingRunBridgeEntry { bridge_tx });

        PendingRunBridge {
            bridge_id,
            bridge_rx,
        }
    }
}

struct PendingRunBridge {
    bridge_id: String,
    bridge_rx: oneshot::Receiver<WebSocket>,
}

struct PendingRunBridgeEntry {
    bridge_tx: oneshot::Sender<WebSocket>,
}

impl RunService {
    pub fn from_env(
        env: &EnvBag,
        session_service: Arc<SessionService>,
        run_store: Arc<dyn RunStore>,
    ) -> Result<Self, AppError> {
        let app_url = read_optional_trimmed_string(env, APP_URL_ENV_NAME).ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {APP_URL_ENV_NAME}"
            ))
        })?;
        let app_url = Url::parse(&app_url).map_err(|error| {
            AppError::config_with_source("ADE_APP_URL is not a valid URL.".to_string(), error)
        })?;

        match app_url.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(AppError::config(
                    "ADE_APP_URL must use http or https.".to_string(),
                ))
            }
        }

        Ok(Self {
            active_runs: ActiveRunManager::default(),
            app_url,
            artifact_store: artifact_store_from_env(env, session_service.session_secret())?,
            bridge_manager: PendingRunBridgeManager::default(),
            run_store,
            session_secret: session_service.session_secret().to_string(),
            session_service,
        })
    }

    pub(crate) async fn attach_bridge_socket(
        &self,
        socket: WebSocket,
        bridge_tx: oneshot::Sender<WebSocket>,
    ) {
        let _ = bridge_tx.send(socket);
    }

    pub(crate) async fn create_run(
        &self,
        scope: Scope,
        request: CreateRunRequest,
    ) -> Result<AsyncRunResponse, AppError> {
        let input_path = validate_input_path(&scope, &request.input_path)?;
        if self.artifact_store.metadata(&input_path).await?.is_none() {
            return Err(AppError::not_found("Run inputPath was not found."));
        }

        let run_id = format!("run_{}", Uuid::new_v4().simple());
        let output_path = output_path_for_run(&scope, &run_id);
        let _ = self
            .run_store
            .create_run(&scope, &run_id, &input_path)
            .await?;
        self.emit_event(
            &run_id,
            None,
            RunEventPayload::Created {
                status: RunStatus::Pending,
            },
        )
        .await?;

        let active = self.active_runs.register(&run_id);
        let service = self.clone();
        let run_id_for_task = run_id.clone();
        let input_path_for_task = input_path.clone();
        let output_path_for_task = output_path.clone();
        let scope_for_task = scope.clone();
        tokio::spawn(async move {
            let _ = service
                .execute_run(
                    scope_for_task,
                    run_id_for_task,
                    input_path_for_task,
                    output_path_for_task,
                    request.timeout_in_seconds,
                    active,
                )
                .await;
        });

        Ok(AsyncRunResponse {
            events_url: run_events_path(
                &scope.workspace_id,
                &scope.config_version_id,
                &run_id,
            ),
            input_path,
            output_path: None,
            run_id,
            status: RunStatus::Pending,
        })
    }

    pub(crate) async fn create_upload(
        &self,
        scope: &Scope,
        request: CreateUploadRequest,
    ) -> Result<CreateUploadResponse, AppError> {
        let upload_id = upload_id();
        let file_path = upload_path_for_file(scope, &upload_id, &request.filename);
        let expires_at = run_access_expiry(RUN_ACCESS_TTL_SECONDS);
        let upload = self
            .artifact_store
            .create_browser_upload_access(&file_path, request.content_type.as_deref(), expires_at)
            .await?;

        Ok(CreateUploadResponse {
            file_path,
            upload: upload_instruction(upload),
            upload_id,
        })
    }

    pub(crate) async fn get_run_detail(
        &self,
        scope: &Scope,
        run_id: &str,
    ) -> Result<RunDetailResponse, AppError> {
        let run = self
            .run_store
            .get_run(scope, run_id)
            .await?
            .ok_or_else(|| AppError::not_found("Run not found."))?;
        Ok(RunDetailResponse {
            error_message: run.error_message.clone(),
            input_path: run.input_path.clone(),
            output_path: run.output_path.clone(),
            phase: run.phase,
            run_id: run.run_id.clone(),
            status: run.status,
            validation_issues: run.validation_issues.clone(),
        })
    }

    pub(crate) async fn cancel_run(&self, scope: &Scope, run_id: &str) -> Result<(), AppError> {
        let Some(mut run) = self.run_store.get_run(scope, run_id).await? else {
            return Err(AppError::not_found("Run not found."));
        };

        if run.status.is_terminal() {
            return Ok(());
        }

        if self.active_runs.cancel(run_id) {
            return Ok(());
        }

        run.status = RunStatus::Cancelled;
        run.error_message = Some("Run cancelled.".to_string());
        self.run_store.save_run(&run).await?;
        self.emit_event(
            run_id,
            None,
            RunEventPayload::Complete {
                final_status: RunStatus::Cancelled,
            },
        )
        .await?;
        Ok(())
    }

    pub(crate) fn claim_bridge(
        &self,
        bridge_id: &str,
        token: &str,
    ) -> Result<oneshot::Sender<WebSocket>, AppError> {
        verify_bridge_token(&self.session_secret, bridge_id, token, unix_time_ms())?;
        self.bridge_manager.claim(bridge_id)
    }

    pub(crate) async fn download_local_artifact(
        &self,
        path: &str,
        token: &str,
    ) -> Result<(String, Vec<u8>), AppError> {
        let normalized = verify_local_artifact_access(
            path,
            &axum::http::Method::GET,
            token,
            &self.session_secret,
            time::OffsetDateTime::now_utc(),
        )?;
        self.artifact_store.download_bytes(&normalized).await
    }

    pub(crate) fn map_public_event(
        run_id: &str,
        event: &RunEvent,
    ) -> Result<(&'static str, String, String), AppError> {
        let (event_name, data) = match event {
            RunEvent::Created { status, .. } => (
                "run.created",
                serde_json::to_string(&PublicRunCreatedEvent {
                    run_id: run_id.to_string(),
                    status: *status,
                }),
            ),
            RunEvent::Status {
                phase,
                state,
                session_guid,
                operation_id,
                timings,
                ..
            } => (
                "run.status",
                serde_json::to_string(&PublicRunStatusEvent {
                    operation_id: operation_id.clone(),
                    phase: *phase,
                    run_id: run_id.to_string(),
                    session_guid: session_guid.clone(),
                    state: state.clone(),
                    timings: timings.clone(),
                }),
            ),
            RunEvent::Log {
                level,
                message,
                phase,
                ..
            } => (
                "run.log",
                serde_json::to_string(&PublicRunLogEvent {
                    level: level.clone(),
                    message: message.clone(),
                    phase: *phase,
                    run_id: run_id.to_string(),
                }),
            ),
            RunEvent::Error {
                phase,
                message,
                retriable,
                ..
            } => (
                "run.error",
                serde_json::to_string(&PublicRunErrorEvent {
                    message: message.clone(),
                    phase: *phase,
                    retriable: *retriable,
                    run_id: run_id.to_string(),
                }),
            ),
            RunEvent::Result {
                output_path,
                validation_issues,
                ..
            } => (
                "run.result",
                serde_json::to_string(&PublicRunResultEvent {
                    output_path: output_path.clone(),
                    run_id: run_id.to_string(),
                    validation_issues: validation_issues.clone(),
                }),
            ),
            RunEvent::Complete { final_status, .. } => (
                "run.completed",
                serde_json::to_string(&PublicRunCompletedEvent {
                    final_status: *final_status,
                    run_id: run_id.to_string(),
                }),
            ),
        };

        Ok((
            event_name,
            event.seq().to_string(),
            data.map_err(|error| {
                AppError::internal_with_source("Failed to encode a run event.", error)
            })?,
        ))
    }

    pub(crate) async fn subscribe_run_events(
        &self,
        scope: &Scope,
        run_id: &str,
        after_seq: Option<i64>,
    ) -> Result<RunEventFeed, AppError> {
        let run = self
            .run_store
            .get_run(scope, run_id)
            .await?
            .ok_or_else(|| AppError::not_found("Run not found."))?;
        let replay = self.run_store.list_events_after(run_id, after_seq).await?;
        let subscription = if run.status.is_terminal() {
            None
        } else {
            self.active_runs.subscribe(run_id)
        };

        Ok(RunEventFeed {
            replay,
            subscription,
        })
    }

    pub(crate) async fn upload_local_artifact(
        &self,
        path: &str,
        token: &str,
        content_type: Option<String>,
        content: Vec<u8>,
    ) -> Result<(), AppError> {
        let normalized = verify_local_artifact_access(
            path,
            &axum::http::Method::PUT,
            token,
            &self.session_secret,
            time::OffsetDateTime::now_utc(),
        )?;
        self.artifact_store
            .upload_bytes(&normalized, content_type.as_deref(), content)
            .await?;
        Ok(())
    }

    async fn consume_run_bridge(
        &self,
        run_id: &str,
        run: &mut StoredRun,
        active: &ActiveRunHandle,
        attempt_session_guid: &mut Option<String>,
        mut bridge_socket: WebSocket,
        mut execution_task: JoinHandle<
            Result<
                crate::session::SessionOperationResult<crate::session::PythonExecution>,
                AppError,
            >,
        >,
    ) -> Result<AttemptSuccess, AttemptFailure> {
        loop {
            match bridge_socket.recv().await {
                Some(Ok(Message::Text(text))) => {
                    match parse_bridge_message(text.as_str()).map_err(|error| AttemptFailure {
                        emitted_error: false,
                        error,
                        phase: Some(RunPhase::InstallPackages),
                        retriable: false,
                    })? {
                        RunBridgeClientMessage::Ready => break,
                        _ => {
                            let _ = send_json(&mut bridge_socket, &RunBridgeServerMessage::Cancel)
                                .await;
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(
                                    StatusCode::BAD_GATEWAY,
                                    "Run bridge must send a ready event before streaming runtime output.",
                                ),
                                phase: Some(RunPhase::InstallPackages),
                                retriable: true,
                            });
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    return Err(AttemptFailure {
                        emitted_error: false,
                        error: AppError::status(
                            StatusCode::BAD_GATEWAY,
                            "Run bridge disconnected before it became ready.",
                        ),
                        phase: Some(RunPhase::InstallPackages),
                        retriable: true,
                    });
                }
                Some(Ok(Message::Binary(_))) => {
                    return Err(AttemptFailure {
                        emitted_error: false,
                        error: AppError::request("Binary run bridge messages are not supported."),
                        phase: Some(RunPhase::InstallPackages),
                        retriable: false,
                    });
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                Some(Err(error)) => {
                    return Err(AttemptFailure {
                        emitted_error: false,
                        error: AppError::internal_with_source(
                            "Failed to read the run bridge websocket.",
                            error,
                        ),
                        phase: Some(RunPhase::InstallPackages),
                        retriable: true,
                    });
                }
            }
        }

        let mut cancel_rx = active.cancel_rx.clone();
        let mut bridge_closed = false;
        let mut execution: Option<
            crate::session::SessionOperationResult<crate::session::PythonExecution>,
        > = None;
        let mut result = None;
        let mut structured_error = None;

        loop {
            if let Some(execution_result) = execution.as_ref() {
                if !matches!(
                    execution_result.value.status.as_str(),
                    "Success" | "Succeeded" | "0"
                ) {
                    if structured_error.is_some() || bridge_closed {
                        let (message, phase, retriable) =
                            structured_error.clone().unwrap_or_else(|| {
                                (
                                    execution_failure_message(&execution_result.value),
                                    run.phase.or(Some(RunPhase::ExecuteRun)),
                                    matches!(run.phase, Some(RunPhase::InstallPackages)),
                                )
                            });
                        return Err(AttemptFailure {
                            emitted_error: structured_error.is_some(),
                            error: AppError::status(StatusCode::BAD_GATEWAY, message),
                            phase,
                            retriable,
                        });
                    }
                } else if let Some(result) = result.clone() {
                    return Ok(result);
                } else if bridge_closed {
                    return Err(AttemptFailure {
                        emitted_error: false,
                        error: AppError::internal(
                            "ADE run bridge did not emit a structured result.",
                        ),
                        phase: Some(RunPhase::PersistOutputs),
                        retriable: false,
                    });
                }
            }

            tokio::select! {
                _ = cancel_rx.changed() => {
                    let _ = send_json(&mut bridge_socket, &RunBridgeServerMessage::Cancel).await;
                    execution_task.abort();
                    return Err(cancelled_failure());
                }
                bridge_message = bridge_socket.recv(), if !bridge_closed => {
                    match bridge_message {
                        Some(Ok(Message::Text(text))) => {
                            let message = parse_bridge_message(text.as_str()).map_err(|error| AttemptFailure {
                                emitted_error: false,
                                error,
                                phase: run.phase.or(Some(RunPhase::InstallPackages)),
                                retriable: false,
                            })?;
                            match message {
                                RunBridgeClientMessage::Ready => {}
                                RunBridgeClientMessage::Status { phase, state } => {
                                    self.handle_runtime_event(
                                        run_id,
                                        run,
                                        Some(active),
                                        RunEventPayload::Status {
                                            phase,
                                            state,
                                            session_guid: run.last_session_guid.clone(),
                                            operation_id: None,
                                            timings: None,
                                        },
                                    ).await.map_err(store_failure)?;
                                }
                                RunBridgeClientMessage::Log { level, message, phase } => {
                                    self.handle_runtime_event(
                                        run_id,
                                        run,
                                        Some(active),
                                        RunEventPayload::Log { level, message, phase },
                                    ).await.map_err(store_failure)?;
                                }
                                RunBridgeClientMessage::Error { phase, message, retriable } => {
                                    structured_error = Some((message.clone(), phase, retriable));
                                    self.handle_runtime_event(
                                        run_id,
                                        run,
                                        Some(active),
                                        RunEventPayload::Error {
                                            phase,
                                            message,
                                            retriable,
                                        },
                                    ).await.map_err(store_failure)?;
                                }
                                RunBridgeClientMessage::Result { output_path, validation_issues } => {
                                    result = Some(AttemptSuccess {
                                        output_path: output_path.clone(),
                                        validation_issues: validation_issues.clone(),
                                    });
                                    self.handle_runtime_event(
                                        run_id,
                                        run,
                                        Some(active),
                                        RunEventPayload::Result {
                                            output_path,
                                            validation_issues,
                                        },
                                    ).await.map_err(store_failure)?;
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            bridge_closed = true;
                        }
                        Some(Ok(Message::Binary(_))) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::request("Binary run bridge messages are not supported."),
                                phase: run.phase.or(Some(RunPhase::InstallPackages)),
                                retriable: false,
                            });
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                        Some(Err(error)) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::internal_with_source("Run bridge failed.", error),
                                phase: run.phase.or(Some(RunPhase::InstallPackages)),
                                retriable: true,
                            });
                        }
                    }
                }
                joined = &mut execution_task, if execution.is_none() => {
                    let execution_result = join_execution_result(joined).map_err(|error| AttemptFailure {
                        emitted_error: structured_error.is_some(),
                        error,
                        phase: run.phase.or(Some(RunPhase::ExecuteRun)),
                        retriable: matches!(run.phase, Some(RunPhase::InstallPackages)),
                    })?;
                    note_session_guid(attempt_session_guid, &execution_result.metadata).map_err(|error| AttemptFailure {
                        emitted_error: structured_error.is_some(),
                        error,
                        phase: run.phase.or(Some(RunPhase::ExecuteRun)),
                        retriable: matches!(run.phase, Some(RunPhase::InstallPackages)),
                    })?;
                    run.last_session_guid = attempt_session_guid.clone();
                    execution = Some(execution_result);
                }
            }
        }
    }

    async fn emit_event(
        &self,
        run_id: &str,
        active: Option<&ActiveRunHandle>,
        event: RunEventPayload,
    ) -> Result<RunEvent, AppError> {
        let event = self.run_store.append_event(run_id, event).await?;
        if let Some(active) = active {
            let _ = active.sender().send(event.clone());
        }
        Ok(event)
    }

    async fn execute_run(
        &self,
        scope: Scope,
        run_id: String,
        input_path: String,
        output_path: String,
        timeout_in_seconds: Option<u64>,
        active: ActiveRunHandle,
    ) -> Result<(), AppError> {
        for attempt in 1..=RUN_MAX_ATTEMPTS {
            if active.is_cancelled() {
                return self.finish_cancelled(&scope, &run_id, Some(&active)).await;
            }

            let failure = match self
                .run_attempt(
                    RunAttempt {
                        attempt,
                        input_path: &input_path,
                        output_path: &output_path,
                        run_id: &run_id,
                        scope: &scope,
                        timeout_in_seconds,
                    },
                    &active,
                )
                .await
            {
                Ok(success) => {
                    return self
                        .finish_success(&scope, &run_id, attempt, success, Some(&active))
                        .await;
                }
                Err(failure) => failure,
            };

            if !(failure.retriable && attempt < RUN_MAX_ATTEMPTS) {
                return self
                    .finish_failure(&scope, &run_id, attempt, &input_path, failure, Some(&active))
                    .await;
            }
        }

        Err(AppError::status(
            StatusCode::BAD_GATEWAY,
            "ADE run failed.",
        ))
    }

    async fn finish_cancelled(
        &self,
        scope: &Scope,
        run_id: &str,
        active: Option<&ActiveRunHandle>,
    ) -> Result<(), AppError> {
        let mut run = self.load_run(scope, run_id).await?;
        run.status = RunStatus::Cancelled;
        run.error_message = Some("Run cancelled.".to_string());
        self.run_store.save_run(&run).await?;
        self.emit_event(
            run_id,
            active,
            RunEventPayload::Complete {
                final_status: RunStatus::Cancelled,
            },
        )
        .await?;
        Err(AppError::request("Run cancelled."))
    }

    async fn finish_failure(
        &self,
        scope: &Scope,
        run_id: &str,
        attempt: i32,
        input_path: &str,
        failure: AttemptFailure,
        active: Option<&ActiveRunHandle>,
    ) -> Result<(), AppError> {
        let mut run = self.load_run(scope, run_id).await?;
        run.attempt_count = attempt;
        run.input_path = input_path.to_string();
        run.phase = failure.phase;
        run.status = if failure.error.to_string() == "Run cancelled." {
            RunStatus::Cancelled
        } else {
            RunStatus::Failed
        };
        run.error_message = Some(failure.error.to_string());
        self.run_store.save_run(&run).await?;

        if !failure.emitted_error {
            self.emit_event(
                run_id,
                active,
                RunEventPayload::Error {
                    phase: failure.phase,
                    message: failure.error.to_string(),
                    retriable: failure.retriable,
                },
            )
            .await?;
        }

        self.emit_event(
            run_id,
            active,
            RunEventPayload::Complete {
                final_status: run.status,
            },
        )
        .await?;
        Err(failure.error)
    }

    async fn finish_success(
        &self,
        scope: &Scope,
        run_id: &str,
        attempt: i32,
        success: AttemptSuccess,
        active: Option<&ActiveRunHandle>,
    ) -> Result<(), AppError> {
        let mut run = self.load_run(scope, run_id).await?;
        run.attempt_count = attempt;
        run.error_message = None;
        run.output_path = Some(success.output_path.clone());
        run.status = RunStatus::Succeeded;
        run.validation_issues = success.validation_issues.clone();
        self.run_store.save_run(&run).await?;
        self.emit_event(
            run_id,
            active,
            RunEventPayload::Complete {
                final_status: RunStatus::Succeeded,
            },
        )
        .await?;
        Ok(())
    }

    async fn handle_runtime_event(
        &self,
        run_id: &str,
        run: &mut StoredRun,
        active: Option<&ActiveRunHandle>,
        event: RunEventPayload,
    ) -> Result<(), AppError> {
        match &event {
            RunEventPayload::Created { status } => {
                run.status = *status;
            }
            RunEventPayload::Status {
                phase,
                state,
                session_guid,
                ..
            } => {
                run.phase = Some(*phase);
                if state == "started" || state == "completed" {
                    run.status = RunStatus::Running;
                }
                if let Some(session_guid) = session_guid {
                    run.last_session_guid = Some(session_guid.clone());
                }
            }
            RunEventPayload::Log { phase, .. } => {
                run.phase = Some(*phase);
            }
            RunEventPayload::Error { phase, message, .. } => {
                run.error_message = Some(message.clone());
                run.phase = *phase;
            }
            RunEventPayload::Result {
                output_path,
                validation_issues,
            } => {
                run.output_path = Some(output_path.clone());
                run.phase = Some(RunPhase::PersistOutputs);
                run.validation_issues = validation_issues.clone();
            }
            RunEventPayload::Complete { final_status } => {
                run.status = *final_status;
            }
        }

        self.run_store.save_run(run).await?;
        self.emit_event(run_id, active, event).await?;
        Ok(())
    }

    async fn load_run(&self, scope: &Scope, run_id: &str) -> Result<StoredRun, AppError> {
        self.run_store
            .get_run(scope, run_id)
            .await?
            .ok_or_else(|| AppError::not_found("Run not found."))
    }

    async fn run_attempt(
        &self,
        attempt: RunAttempt<'_>,
        active: &ActiveRunHandle,
    ) -> Result<AttemptSuccess, AttemptFailure> {
        let runtime = self
            .session_service
            .runtime_artifacts(attempt.scope)
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::InstallPackages),
                retriable: false,
            })?;
        let mut run = self
            .load_run(attempt.scope, attempt.run_id)
            .await
            .map_err(store_failure)?;
        run.attempt_count = attempt.attempt;
        run.error_message = None;
        run.output_path = None;
        run.phase = None;
        run.status = RunStatus::Running;
        run.validation_issues.clear();
        self.run_store.save_run(&run).await.map_err(store_failure)?;

        let mut attempt_session_guid = run.last_session_guid.clone();
        self.upload_runtime_artifacts(attempt.scope, &runtime, &mut attempt_session_guid)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::InstallPackages),
                retriable: true,
            })?;

        run.last_session_guid = attempt_session_guid.clone();
        self.run_store.save_run(&run).await.map_err(store_failure)?;

        if active.is_cancelled() {
            return Err(cancelled_failure());
        }

        let pending = self.bridge_manager.create();
        let bridge_url = self.build_bridge_url(&pending.bridge_id).map_err(|error| AttemptFailure {
            emitted_error: false,
            error,
            phase: Some(RunPhase::InstallPackages),
            retriable: false,
        })?;
        let access_expires_at =
            run_access_expiry(attempt.timeout_in_seconds.unwrap_or(RUN_ACCESS_TTL_SECONDS));
        let input_download = self
            .artifact_store
            .create_download_access(attempt.input_path, access_expires_at)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::InstallPackages),
                retriable: false,
            })?;
        let output_upload = self
            .artifact_store
            .create_upload_access(attempt.output_path, None, access_expires_at)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::PersistOutputs),
                retriable: false,
            })?;
        let bootstrap = render_bootstrap_code(&RunBootstrapConfig {
            config_package_name: runtime.config_package_name,
            config_version: runtime.config_version.clone(),
            config_wheel_path: session_path(&runtime.config_filename),
            bridge_url,
            engine_package_name: runtime.engine_package_name,
            engine_version: runtime.engine_version.clone(),
            engine_wheel_path: session_path(&runtime.engine_filename),
            input_download: bootstrap_artifact_access(&self.app_url, input_download).map_err(
                |error| AttemptFailure {
                    emitted_error: false,
                    error,
                    phase: Some(RunPhase::InstallPackages),
                    retriable: false,
                },
            )?,
            install_lock_path: runtime.install_lock_path.clone(),
            local_input_path: local_input_path(attempt.input_path),
            local_output_dir: local_output_dir(attempt.run_id),
            output_path: attempt.output_path.to_string(),
            output_upload: bootstrap_artifact_access(&self.app_url, output_upload).map_err(
                |error| AttemptFailure {
                    emitted_error: false,
                    error,
                    phase: Some(RunPhase::PersistOutputs),
                    retriable: false,
                },
            )?,
        })
        .map_err(|error| AttemptFailure {
            emitted_error: false,
            error,
            phase: Some(RunPhase::InstallPackages),
            retriable: false,
        })?;

        let session_service = Arc::clone(&self.session_service);
        let execution_timeout = attempt
            .timeout_in_seconds
            .unwrap_or(RUN_EXECUTION_TIMEOUT_SECONDS);
        let scope_for_execution = attempt.scope.clone();
        let execution_task = tokio::spawn(async move {
            session_service
                .execute_inline_python_detailed(
                    &scope_for_execution,
                    bootstrap,
                    Some(execution_timeout),
                )
                .await
        });

        let bridge_socket = self
            .wait_for_run_bridge(pending, active)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::InstallPackages),
                retriable: true,
            })?;
        self.consume_run_bridge(
            attempt.run_id,
            &mut run,
            active,
            &mut attempt_session_guid,
            bridge_socket,
            execution_task,
        )
        .await
    }

    async fn upload_runtime_artifacts(
        &self,
        scope: &Scope,
        runtime: &SessionRuntimeArtifacts,
        attempt_session_guid: &mut Option<String>,
    ) -> Result<(), AppError> {
        let engine_upload = self
            .session_service
            .upload_session_file_detailed(
                scope,
                &runtime.engine_filename,
                Some("application/octet-stream".to_string()),
                runtime.engine_wheel_bytes.clone(),
            )
            .await?;
        note_session_guid(attempt_session_guid, &engine_upload.metadata)?;

        let config_upload = self
            .session_service
            .upload_session_file_detailed(
                scope,
                &runtime.config_filename,
                Some("application/octet-stream".to_string()),
                runtime.config_wheel_bytes.clone(),
            )
            .await?;
        note_session_guid(attempt_session_guid, &config_upload.metadata)?;
        Ok(())
    }

    async fn wait_for_run_bridge(
        &self,
        pending: PendingRunBridge,
        active: &ActiveRunHandle,
    ) -> Result<WebSocket, AppError> {
        let timeout = tokio::time::sleep(BRIDGE_READY_TIMEOUT);
        tokio::pin!(timeout);
        let bridge_rx = pending.bridge_rx;
        tokio::pin!(bridge_rx);
        let mut cancel_rx = active.cancel_rx.clone();

        tokio::select! {
            _ = cancel_rx.changed() => {
                self.bridge_manager.cancel(&pending.bridge_id);
                Err(AppError::request("Run cancelled."))
            }
            result = &mut bridge_rx => {
                result.map_err(|_| AppError::status(StatusCode::BAD_GATEWAY, "Run bridge did not connect."))
            }
            _ = &mut timeout => {
                self.bridge_manager.cancel(&pending.bridge_id);
                Err(AppError::status(StatusCode::BAD_GATEWAY, "Timed out waiting for the run bridge to connect."))
            }
        }
    }

    fn build_bridge_url(&self, bridge_id: &str) -> Result<String, AppError> {
        let expires_at_ms = unix_time_ms() + BRIDGE_TOKEN_TTL_MS;
        let token = create_bridge_token(&self.session_secret, bridge_id, expires_at_ms);
        let mut bridge_url = self.app_url.clone();
        let scheme = match bridge_url.scheme() {
            "http" => "ws",
            "https" => "wss",
            _ => {
                return Err(AppError::config(
                    "ADE_APP_URL must use http or https.".to_string(),
                ))
            }
        };
        bridge_url
            .set_scheme(scheme)
            .map_err(|()| AppError::internal("Failed to derive the run bridge URL."))?;
        bridge_url.set_path(&format!("/api/internal/run-bridges/{bridge_id}"));
        bridge_url.set_query(None);
        bridge_url.query_pairs_mut().append_pair("token", &token);
        Ok(bridge_url.to_string())
    }
}

fn bootstrap_artifact_access(
    app_url: &Url,
    access: ArtifactAccessGrant,
) -> Result<BootstrapArtifactAccess, AppError> {
    let resolved_url = resolve_access_url(app_url, &access)?;
    Ok(BootstrapArtifactAccess {
        headers: access.headers.into_iter().collect(),
        method: access.method.to_string(),
        url: resolved_url,
    })
}

fn cancelled_failure() -> AttemptFailure {
    AttemptFailure {
        emitted_error: true,
        error: AppError::request("Run cancelled."),
        phase: None,
        retriable: false,
    }
}

fn create_bridge_token(secret: &str, bridge_id: &str, expires_at_ms: u64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(bridge_id.as_bytes());
    mac.update(b":");
    mac.update(expires_at_ms.to_string().as_bytes());
    format!(
        "{expires_at_ms}.{}",
        hex::encode(mac.finalize().into_bytes())
    )
}

fn execution_failure_message(execution: &crate::session::PythonExecution) -> String {
    if !execution.stderr.trim().is_empty() {
        return execution.stderr.trim().to_string();
    }
    if !execution.stdout.trim().is_empty() {
        return execution.stdout.trim().to_string();
    }
    format!("ADE run execution failed with status {}.", execution.status)
}

fn join_execution_result<T>(
    result: Result<Result<T, AppError>, tokio::task::JoinError>,
) -> Result<T, AppError> {
    match result {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Err(AppError::request("Run cancelled.")),
        Err(error) => Err(AppError::internal_with_source(
            "Run execution task failed to join.",
            error,
        )),
    }
}

fn local_input_path(input_path: &str) -> String {
    let filename = uploaded_filename(input_path).unwrap_or_else(|_| "input.bin".to_string());
    format!("/mnt/data/work/input/{filename}")
}

fn local_output_dir(run_id: &str) -> String {
    format!("/mnt/data/runs/{run_id}/output")
}

fn note_session_guid(
    current_session_guid: &mut Option<String>,
    metadata: &SessionOperationMetadata,
) -> Result<(), AppError> {
    let Some(session_guid) = metadata.session_guid.as_ref() else {
        return Ok(());
    };

    if let Some(current) = current_session_guid.as_ref()
        && current != session_guid
    {
        return Err(AppError::status(
            StatusCode::BAD_GATEWAY,
            "The Azure session was recycled while the run was in progress.",
        ));
    }

    *current_session_guid = Some(session_guid.clone());
    Ok(())
}

fn parse_bridge_message(text: &str) -> Result<RunBridgeClientMessage, AppError> {
    serde_json::from_str::<RunBridgeClientMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid run bridge message: {error}")))
}

fn render_bootstrap_code(config: &RunBootstrapConfig) -> Result<String, AppError> {
    if !BOOTSTRAP_TEMPLATE.contains("{{CONFIG_JSON}}") {
        return Err(AppError::internal(
            "Run bootstrap template is missing the CONFIG_JSON placeholder.",
        ));
    }

    let config_json = serde_json::to_string(config).map_err(|error| {
        AppError::internal_with_source("Failed to encode the run bootstrap configuration.", error)
    })?;
    let encoded = serde_json::to_string(&config_json).map_err(|error| {
        AppError::internal_with_source("Failed to encode the run bootstrap JSON string.", error)
    })?;
    Ok(BOOTSTRAP_TEMPLATE.replace("{{CONFIG_JSON}}", &encoded))
}

fn run_access_expiry(ttl_seconds: u64) -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc() + time::Duration::seconds(ttl_seconds as i64)
}

fn run_events_path(workspace_id: &str, config_version_id: &str, run_id: &str) -> String {
    format!("/api/workspaces/{workspace_id}/configs/{config_version_id}/runs/{run_id}/events")
}

async fn send_json(socket: &mut WebSocket, payload: &impl Serialize) -> Result<(), AppError> {
    let message = serde_json::to_string(payload).map_err(|error| {
        AppError::internal_with_source("Failed to encode a websocket payload.", error)
    })?;
    socket
        .send(Message::Text(message.into()))
        .await
        .map_err(|error| AppError::internal_with_source("Failed to write to a websocket.", error))
}

fn session_path(relative_path: &str) -> String {
    format!("/mnt/data/{}", relative_path.trim_start_matches('/'))
}

fn store_failure(error: AppError) -> AttemptFailure {
    AttemptFailure {
        emitted_error: false,
        error,
        phase: None,
        retriable: false,
    }
}

fn upload_instruction(access: ArtifactAccessGrant) -> UploadInstruction {
    UploadInstruction {
        expires_at: access.expires_at,
        headers: access.headers.into_iter().collect(),
        method: access.method.to_string(),
        url: access.url,
    }
}

fn uploaded_filename(filename: &str) -> Result<String, AppError> {
    let name = std::path::Path::new(filename.trim())
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::request("Uploaded file must include a valid filename."))?;

    if name.is_empty() {
        return Err(AppError::request(
            "Uploaded file must include a valid filename.",
        ));
    }

    Ok(name.to_string())
}

fn verify_bridge_token(
    secret: &str,
    bridge_id: &str,
    token: &str,
    now_ms: u64,
) -> Result<(), AppError> {
    let Some((expires_at_ms, signature_hex)) = token.split_once('.') else {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Invalid run bridge token.",
        ));
    };
    let expires_at_ms = expires_at_ms
        .parse::<u64>()
        .map_err(|_| AppError::status(StatusCode::UNAUTHORIZED, "Invalid run bridge token."))?;
    if now_ms > expires_at_ms {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Run bridge token expired.",
        ));
    }

    let signature = hex::decode(signature_hex)
        .map_err(|_| AppError::status(StatusCode::UNAUTHORIZED, "Invalid run bridge token."))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is valid");
    mac.update(bridge_id.as_bytes());
    mac.update(b":");
    mac.update(expires_at_ms.to_string().as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| AppError::status(StatusCode::UNAUTHORIZED, "Invalid run bridge token."))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{RunBootstrapConfig, local_input_path, render_bootstrap_code, run_events_path, uploaded_filename};

    #[test]
    fn bootstrap_code_contains_direct_access_and_public_output_path() {
        let code = render_bootstrap_code(&RunBootstrapConfig {
            config_package_name: "ade-config",
            config_version: "1.0.0".to_string(),
            config_wheel_path: "/mnt/data/config.whl".to_string(),
            bridge_url: "wss://example.com/bridge".to_string(),
            engine_package_name: "ade-engine",
            engine_version: "1.0.0".to_string(),
            engine_wheel_path: "/mnt/data/engine.whl".to_string(),
            input_download: super::BootstrapArtifactAccess {
                headers: HashMap::new(),
                method: "GET".to_string(),
                url: "https://example.com/input".to_string(),
            },
            install_lock_path: "/mnt/data/.lock".to_string(),
            local_input_path: "/mnt/data/work/input/input.xlsx".to_string(),
            local_output_dir: "/mnt/data/runs/run-1/output".to_string(),
            output_path: "workspaces/workspace-a/configs/config-v1/runs/run-1/output/normalized.xlsx".to_string(),
            output_upload: super::BootstrapArtifactAccess {
                headers: HashMap::new(),
                method: "PUT".to_string(),
                url: "https://example.com/output".to_string(),
            },
        })
        .unwrap();

        assert!(code.contains("urllib.request"));
        assert!(code.contains("normalized.xlsx"));
        assert!(code.contains("https://example.com/input"));
    }

    #[test]
    fn local_input_paths_use_safe_basenames() {
        assert_eq!(
            local_input_path("workspaces/a/configs/b/uploads/u/input.xlsx"),
            "/mnt/data/work/input/input.xlsx"
        );
    }

    #[test]
    fn run_events_urls_are_stable() {
        assert_eq!(
            run_events_path("workspace-a", "config-v1", "run-1"),
            "/api/workspaces/workspace-a/configs/config-v1/runs/run-1/events"
        );
    }

    #[test]
    fn uploaded_filenames_are_reduced_to_a_safe_basename() {
        assert_eq!(uploaded_filename("/tmp/input.xlsx").unwrap(), "input.xlsx");
    }
}
