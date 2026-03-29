use std::{
    collections::HashMap,
    path::Path,
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
    artifacts::{ArtifactStoreHandle, artifact_store_from_env, normalize_artifact_path},
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    run_store::{
        RunEvent, RunEventPayload, RunPhase, RunStatus, RunStore, RunStoreHandle, StoredRun,
    },
    session::{
        CreateRunRequest, RunResponse, RunValidationIssue, Scope, SessionOperationMetadata,
        SessionRuntimeArtifacts, SessionService,
    },
    unix_time_ms,
};

const APP_URL_ENV_NAME: &str = "ADE_APP_URL";
const BRIDGE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const BRIDGE_TOKEN_TTL_MS: u64 = 60_000;
const RUN_EXECUTION_TIMEOUT_SECONDS: u64 = 220;
const RUN_EVENT_SENTINEL_PREFIX: &str = "__ADE_RUN_EVENT__=";
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
#[serde(rename_all = "camelCase")]
pub(crate) struct AsyncRunResponse {
    pub events_url: String,
    pub run_id: String,
    pub status: RunStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDetailResponse {
    pub attempt_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub input_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_session_guid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<RunPhase>,
    pub run_id: String,
    pub status: RunStatus,
    pub validation_issues: Vec<RunValidationIssue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSocketHello {
    #[serde(rename = "type")]
    message_type: &'static str,
    last_seq: i64,
    run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_guid: Option<String>,
    status: RunStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSocketError {
    #[serde(rename = "type")]
    message_type: &'static str,
    message: String,
    retriable: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum BrowserRunClientMessage {
    Attach {
        #[serde(default)]
        last_seen_seq: Option<i64>,
    },
    Cancel,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum RunBridgeClientMessage {
    Ready,
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

#[derive(Debug)]
struct AttemptFailure {
    emitted_error: bool,
    error: AppError,
    #[allow(dead_code)]
    output_manifest_committed: bool,
    phase: Option<RunPhase>,
    retriable: bool,
}

struct RunAttemptContext<'a> {
    attempt_session_guid: &'a mut Option<String>,
    input_path: &'a str,
    run: &'a mut StoredRun,
    run_id: &'a str,
    runtime: &'a SessionRuntimeArtifacts,
    scope: &'a Scope,
    timeout_in_seconds: Option<u64>,
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

    fn remove(&self, run_id: &str) {
        self.inner
            .lock()
            .expect("active run lock poisoned")
            .remove(run_id);
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

    fn claim(&self, bridge_id: &str) -> Result<oneshot::Sender<WebSocket>, AppError> {
        self.inner
            .lock()
            .expect("pending run bridge lock poisoned")
            .remove(bridge_id)
            .map(|entry| entry.bridge_tx)
            .ok_or_else(|| AppError::not_found("Run bridge not found."))
    }

    fn cancel(&self, bridge_id: &str) {
        self.inner
            .lock()
            .expect("pending run bridge lock poisoned")
            .remove(bridge_id);
    }
}

struct PendingRunBridgeEntry {
    bridge_tx: oneshot::Sender<WebSocket>,
}

struct PendingRunBridge {
    bridge_id: String,
    bridge_rx: oneshot::Receiver<WebSocket>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunBootstrapConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    bridge_url: Option<String>,
    config_package_name: &'static str,
    config_version: String,
    config_wheel_path: String,
    engine_package_name: &'static str,
    engine_version: String,
    engine_wheel_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_sentinel_prefix: Option<&'static str>,
    input_path: String,
    install_lock_path: String,
    output_dir: String,
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
                ));
            }
        }

        Ok(Self {
            active_runs: ActiveRunManager::default(),
            app_url,
            artifact_store: artifact_store_from_env(env)?,
            bridge_manager: PendingRunBridgeManager::default(),
            run_store,
            session_secret: session_service.session_secret().to_string(),
            session_service,
        })
    }

    pub(crate) async fn upload_file(
        &self,
        scope: &Scope,
        filename: String,
        content_type: Option<String>,
        content: Vec<u8>,
    ) -> Result<crate::session::SessionFile, AppError> {
        self.artifact_store
            .upload(
                scope,
                &uploaded_filename(&filename)?,
                content_type.as_deref(),
                content,
            )
            .await
    }

    pub(crate) async fn list_files(
        &self,
        scope: &Scope,
    ) -> Result<Vec<crate::session::SessionFile>, AppError> {
        self.artifact_store.list(scope).await
    }

    pub(crate) async fn download_file(
        &self,
        scope: &Scope,
        path: &str,
    ) -> Result<(String, Vec<u8>), AppError> {
        self.artifact_store.download(scope, path).await
    }

    pub(crate) async fn create_sync_run(
        &self,
        scope: Scope,
        request: CreateRunRequest,
    ) -> Result<RunResponse, AppError> {
        let input_path = normalize_artifact_path(&request.input_path)?;
        let run_id = Uuid::new_v4().to_string();
        let _ = self
            .run_store
            .create_run(&scope, &run_id, &input_path)
            .await?;

        self.execute_run(scope, run_id, input_path, request.timeout_in_seconds, None)
            .await
    }

    pub(crate) async fn create_async_run(
        &self,
        scope: Scope,
        request: CreateRunRequest,
    ) -> Result<AsyncRunResponse, AppError> {
        let input_path = normalize_artifact_path(&request.input_path)?;
        let run_id = Uuid::new_v4().to_string();
        let _ = self
            .run_store
            .create_run(&scope, &run_id, &input_path)
            .await?;
        let active = self.active_runs.register(&run_id);
        let service = self.clone();
        let run_id_for_task = run_id.clone();
        let scope_for_response = scope.clone();

        tokio::spawn(async move {
            let _ = service
                .execute_run(
                    scope,
                    run_id_for_task,
                    input_path,
                    request.timeout_in_seconds,
                    Some(active),
                )
                .await;
        });

        Ok(AsyncRunResponse {
            events_url: run_events_path(
                scope_for_response.workspace_id.as_str(),
                scope_for_response.config_version_id.as_str(),
                &run_id,
            ),
            run_id,
            status: RunStatus::Pending,
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
            attempt_count: run.attempt_count,
            error_message: run.error_message.clone(),
            input_path: run.input_path.clone(),
            last_event_seq: self.run_store.last_event_seq(&run.run_id).await?,
            last_session_guid: run.last_session_guid.clone(),
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

    pub(crate) async fn stream_run_events(
        &self,
        scope: Scope,
        mut browser_socket: WebSocket,
        run_id: String,
    ) {
        if let Err(error) = self
            .stream_run_events_inner(scope, &run_id, &mut browser_socket)
            .await
        {
            let _ = send_json(
                &mut browser_socket,
                &RunSocketError {
                    message_type: "error",
                    message: error.to_string(),
                    retriable: false,
                },
            )
            .await;
            let _ = browser_socket.send(Message::Close(None)).await;
            return;
        }

        let _ = browser_socket.send(Message::Close(None)).await;
    }

    pub(crate) fn claim_bridge(
        &self,
        bridge_id: &str,
        token: &str,
    ) -> Result<oneshot::Sender<WebSocket>, AppError> {
        verify_bridge_token(&self.session_secret, bridge_id, token, unix_time_ms())?;
        self.bridge_manager.claim(bridge_id)
    }

    pub(crate) async fn attach_bridge_socket(
        &self,
        socket: WebSocket,
        bridge_tx: oneshot::Sender<WebSocket>,
    ) {
        let _ = bridge_tx.send(socket);
    }

    async fn stream_run_events_inner(
        &self,
        scope: Scope,
        run_id: &str,
        browser_socket: &mut WebSocket,
    ) -> Result<(), AppError> {
        let attach = read_attach_message(browser_socket).await?;
        let run = self
            .run_store
            .get_run(&scope, run_id)
            .await?
            .ok_or_else(|| AppError::not_found("Run not found."))?;
        let last_seq = self.run_store.last_event_seq(run_id).await?.unwrap_or(0);
        send_json(
            browser_socket,
            &RunSocketHello {
                message_type: "hello",
                last_seq,
                run_id: run.run_id.clone(),
                session_guid: run.last_session_guid.clone(),
                status: run.status,
            },
        )
        .await?;

        let mut delivered_seq = attach.last_seen_seq.unwrap_or(0);
        let replay = self
            .run_store
            .list_events_after(run_id, attach.last_seen_seq)
            .await?;
        for event in replay {
            delivered_seq = event.seq();
            send_json(browser_socket, &event).await?;
        }

        let mut receiver = self.active_runs.subscribe(run_id);
        if receiver.is_none() || run.status.is_terminal() {
            return Ok(());
        }
        let mut receiver = receiver.take().expect("receiver present");

        loop {
            tokio::select! {
                browser_message = browser_socket.recv() => {
                    match browser_message {
                        Some(Ok(Message::Text(text))) => {
                            match parse_browser_message(text.as_str())? {
                                BrowserRunClientMessage::Attach { .. } => {}
                                BrowserRunClientMessage::Cancel => {
                                    self.cancel_run(&scope, run_id).await?;
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => return Ok(()),
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Binary(_))) => {
                            return Err(AppError::request("Binary run event messages are not supported."));
                        }
                        Some(Err(error)) => {
                            return Err(AppError::internal_with_source("Failed to read from the run events websocket.", error));
                        }
                    }
                }
                event = receiver.recv() => {
                    match event {
                        Ok(event) => {
                            let next_seq = event.seq();
                            send_json(browser_socket, &event).await?;
                            if matches!(event, RunEvent::Complete { .. }) {
                                return Ok(());
                            }
                            delivered_seq = next_seq;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            let replay = self
                                .run_store
                                .list_events_after(run_id, Some(delivered_seq))
                                .await?;
                            for event in replay {
                                send_json(browser_socket, &event).await?;
                            }
                            return Ok(());
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let replay = self
                                .run_store
                                .list_events_after(run_id, Some(delivered_seq))
                                .await?;
                            for event in replay {
                                let next_seq = event.seq();
                                send_json(browser_socket, &event).await?;
                                if matches!(event, RunEvent::Complete { .. }) {
                                    return Ok(());
                                }
                                delivered_seq = next_seq;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn execute_run(
        &self,
        scope: Scope,
        run_id: String,
        input_path: String,
        timeout_in_seconds: Option<u64>,
        active: Option<ActiveRunHandle>,
    ) -> Result<RunResponse, AppError> {
        let mut last_error = None;

        for attempt in 1..=RUN_MAX_ATTEMPTS {
            if let Some(active) = active.as_ref()
                && active.is_cancelled()
            {
                return self.finish_cancelled(&scope, &run_id, Some(active)).await;
            }

            let failure = match self
                .run_attempt(
                    &scope,
                    &run_id,
                    &input_path,
                    attempt,
                    timeout_in_seconds,
                    active.as_ref(),
                )
                .await
            {
                Ok(success) => {
                    return self
                        .finish_success(
                            &scope,
                            &run_id,
                            attempt,
                            &input_path,
                            success,
                            active.as_ref(),
                        )
                        .await;
                }
                Err(failure) => failure,
            };

            last_error = Some(failure.error.to_string());
            if !(failure.retriable && attempt < RUN_MAX_ATTEMPTS) {
                return self
                    .finish_failure(
                        &scope,
                        &run_id,
                        attempt,
                        &input_path,
                        failure,
                        active.as_ref(),
                    )
                    .await;
            }
        }

        Err(AppError::status(
            StatusCode::BAD_GATEWAY,
            last_error.unwrap_or_else(|| "ADE run failed.".to_string()),
        ))
    }

    async fn run_attempt(
        &self,
        scope: &Scope,
        run_id: &str,
        input_path: &str,
        attempt: i32,
        timeout_in_seconds: Option<u64>,
        active: Option<&ActiveRunHandle>,
    ) -> Result<AttemptSuccess, AttemptFailure> {
        let runtime = self
            .session_service
            .runtime_artifacts(scope)
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                output_manifest_committed: false,
                phase: Some(RunPhase::UploadArtifacts),
                retriable: false,
            })?;
        let mut run = self
            .load_run(scope, run_id)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                output_manifest_committed: false,
                phase: None,
                retriable: false,
            })?;
        run.attempt_count = attempt;
        run.status = RunStatus::Running;
        run.phase = Some(RunPhase::UploadArtifacts);
        run.error_message = None;
        run.output_path = None;
        run.validation_issues.clear();
        self.run_store.save_run(&run).await.map_err(store_failure)?;

        let mut attempt_session_guid = run.last_session_guid.clone();
        self.emit_status(&mut run, active, RunPhase::UploadArtifacts, "started", None)
            .await
            .map_err(store_failure)?;

        let input_bytes = self
            .artifact_store
            .download(scope, input_path)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                output_manifest_committed: false,
                phase: Some(RunPhase::UploadArtifacts),
                retriable: false,
            })?;

        self.upload_session_artifacts(
            scope,
            input_path,
            input_bytes,
            &runtime,
            &mut attempt_session_guid,
        )
        .await
        .map_err(|error| AttemptFailure {
            emitted_error: false,
            error,
            output_manifest_committed: false,
            phase: Some(RunPhase::UploadArtifacts),
            retriable: true,
        })?;

        run.last_session_guid = attempt_session_guid.clone();
        self.emit_status(
            &mut run,
            active,
            RunPhase::UploadArtifacts,
            "completed",
            None,
        )
        .await
        .map_err(store_failure)?;

        if let Some(active) = active
            && active.is_cancelled()
        {
            return Err(cancelled_failure());
        }

        let context = RunAttemptContext {
            attempt_session_guid: &mut attempt_session_guid,
            input_path,
            run: &mut run,
            run_id,
            runtime: &runtime,
            scope,
            timeout_in_seconds,
        };

        if let Some(active) = active {
            self.run_attempt_async(context, active).await
        } else {
            self.run_attempt_sync(context).await
        }
    }

    async fn run_attempt_sync(
        &self,
        context: RunAttemptContext<'_>,
    ) -> Result<AttemptSuccess, AttemptFailure> {
        let RunAttemptContext {
            attempt_session_guid,
            input_path,
            run,
            run_id,
            runtime,
            scope,
            timeout_in_seconds,
        } = context;
        let bootstrap = render_bootstrap_code(&RunBootstrapConfig {
            bridge_url: None,
            config_package_name: runtime.config_package_name,
            config_version: runtime.config_version.clone(),
            config_wheel_path: session_path(&runtime.config_filename),
            engine_package_name: runtime.engine_package_name,
            engine_version: runtime.engine_version.clone(),
            engine_wheel_path: session_path(&runtime.engine_filename),
            event_sentinel_prefix: Some(RUN_EVENT_SENTINEL_PREFIX),
            input_path: session_path(input_path),
            install_lock_path: runtime.install_lock_path.clone(),
            output_dir: session_output_dir(run_id),
        })
        .map_err(|error| AttemptFailure {
            emitted_error: false,
            error,
            output_manifest_committed: false,
            phase: None,
            retriable: false,
        })?;

        let execution = self
            .session_service
            .execute_inline_python_detailed(
                scope,
                bootstrap,
                Some(timeout_in_seconds.unwrap_or(RUN_EXECUTION_TIMEOUT_SECONDS)),
            )
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                output_manifest_committed: false,
                phase: Some(run.phase.unwrap_or(RunPhase::InstallPackages)),
                retriable: true,
            })?;
        note_session_guid(attempt_session_guid, &execution.metadata).map_err(|error| {
            AttemptFailure {
                emitted_error: false,
                error,
                output_manifest_committed: false,
                phase: Some(RunPhase::UploadArtifacts),
                retriable: true,
            }
        })?;
        run.last_session_guid = attempt_session_guid.clone();

        let parsed =
            parse_sync_output(&execution.value.stdout).map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                output_manifest_committed: false,
                phase: Some(run.phase.unwrap_or(RunPhase::InstallPackages)),
                retriable: false,
            })?;

        for event in &parsed.events {
            self.handle_runtime_event(run_id, run, None, event.clone())
                .await
                .map_err(store_failure)?;
        }

        if !matches!(
            execution.value.status.as_str(),
            "Success" | "Succeeded" | "0"
        ) {
            let last_phase = parsed.events.iter().rev().find_map(|event| match event {
                RunEventPayload::Status { phase, .. } => Some(*phase),
                RunEventPayload::Log { phase, .. } => Some(*phase),
                RunEventPayload::Error { phase, .. } => *phase,
                RunEventPayload::Result { .. } | RunEventPayload::Complete { .. } => None,
            });
            let structured_error = parsed.events.iter().rev().find_map(|event| match event {
                RunEventPayload::Error {
                    message,
                    phase,
                    retriable,
                } => Some((message.clone(), *phase, *retriable)),
                _ => None,
            });
            let emitted_error = structured_error.is_some();
            let (message, phase, retriable) = structured_error.clone().unwrap_or_else(|| {
                (
                    execution_failure_message(&execution.value),
                    last_phase,
                    matches!(last_phase, Some(RunPhase::InstallPackages)),
                )
            });
            return Err(AttemptFailure {
                emitted_error,
                error: AppError::status(StatusCode::BAD_GATEWAY, message),
                output_manifest_committed: parsed.result.is_some(),
                phase,
                retriable,
            });
        }

        let result = parsed.result.ok_or_else(|| AttemptFailure {
            emitted_error: false,
            error: AppError::internal("ADE run did not emit a structured result."),
            output_manifest_committed: false,
            phase: Some(RunPhase::ExecuteRun),
            retriable: false,
        })?;

        self.emit_status(
            run,
            None,
            RunPhase::ExecuteRun,
            "completed",
            Some(execution.metadata),
        )
        .await
        .map_err(store_failure)?;

        Ok(AttemptSuccess {
            output_path: result.output_path,
            validation_issues: result.validation_issues,
        })
    }

    async fn run_attempt_async(
        &self,
        context: RunAttemptContext<'_>,
        active: &ActiveRunHandle,
    ) -> Result<AttemptSuccess, AttemptFailure> {
        let RunAttemptContext {
            attempt_session_guid,
            input_path,
            run,
            run_id,
            runtime,
            scope,
            timeout_in_seconds,
        } = context;
        let pending = self.bridge_manager.create();
        let bridge_url =
            self.build_bridge_url(&pending.bridge_id)
                .map_err(|error| AttemptFailure {
                    emitted_error: false,
                    error,
                    output_manifest_committed: false,
                    phase: Some(RunPhase::InstallPackages),
                    retriable: false,
                })?;
        let bootstrap = render_bootstrap_code(&RunBootstrapConfig {
            bridge_url: Some(bridge_url),
            config_package_name: runtime.config_package_name,
            config_version: runtime.config_version.clone(),
            config_wheel_path: session_path(&runtime.config_filename),
            engine_package_name: runtime.engine_package_name,
            engine_version: runtime.engine_version.clone(),
            engine_wheel_path: session_path(&runtime.engine_filename),
            event_sentinel_prefix: None,
            input_path: session_path(input_path),
            install_lock_path: runtime.install_lock_path.clone(),
            output_dir: session_output_dir(run_id),
        })
        .map_err(|error| AttemptFailure {
            emitted_error: false,
            error,
            output_manifest_committed: false,
            phase: Some(RunPhase::InstallPackages),
            retriable: false,
        })?;
        let session_service = Arc::clone(&self.session_service);
        let scope_for_exec = scope.clone();
        let execution_task = tokio::spawn(async move {
            session_service
                .execute_inline_python_detailed(
                    &scope_for_exec,
                    bootstrap,
                    Some(timeout_in_seconds.unwrap_or(RUN_EXECUTION_TIMEOUT_SECONDS)),
                )
                .await
        });

        let bridge_socket = self
            .wait_for_run_bridge(pending, active, run_id)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                output_manifest_committed: false,
                phase: Some(RunPhase::InstallPackages),
                retriable: true,
            })?;

        self.consume_run_bridge(
            run_id,
            run,
            active,
            attempt_session_guid,
            bridge_socket,
            execution_task,
        )
        .await
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
                        output_manifest_committed: false,
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
                                output_manifest_committed: false,
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
                        output_manifest_committed: false,
                        phase: Some(RunPhase::InstallPackages),
                        retriable: true,
                    });
                }
                Some(Ok(Message::Binary(_))) => {
                    return Err(AttemptFailure {
                        emitted_error: false,
                        error: AppError::request("Binary run bridge messages are not supported."),
                        output_manifest_committed: false,
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
                        output_manifest_committed: false,
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
        let mut structured_error = None;
        let mut result = None;

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
                                    Some(run.phase.unwrap_or(RunPhase::ExecuteRun)),
                                    matches!(run.phase, Some(RunPhase::InstallPackages)),
                                )
                            });
                        return Err(AttemptFailure {
                            emitted_error: structured_error.is_some(),
                            error: AppError::status(StatusCode::BAD_GATEWAY, message),
                            output_manifest_committed: result.is_some(),
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
                        output_manifest_committed: false,
                        phase: Some(RunPhase::ExecuteRun),
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
                                output_manifest_committed: result.is_some(),
                                phase: Some(run.phase.unwrap_or(RunPhase::InstallPackages)),
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
                                output_manifest_committed: result.is_some(),
                                phase: Some(run.phase.unwrap_or(RunPhase::InstallPackages)),
                                retriable: false,
                            });
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                        Some(Err(error)) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::internal_with_source("Run bridge failed.", error),
                                output_manifest_committed: result.is_some(),
                                phase: Some(run.phase.unwrap_or(RunPhase::InstallPackages)),
                                retriable: true,
                            });
                        }
                    }
                }
                joined = &mut execution_task, if execution.is_none() => {
                    let execution_result = join_execution_result(joined).map_err(|error| AttemptFailure {
                        emitted_error: structured_error.is_some(),
                        error,
                        output_manifest_committed: result.is_some(),
                        phase: Some(run.phase.unwrap_or(RunPhase::ExecuteRun)),
                        retriable: matches!(run.phase, Some(RunPhase::InstallPackages)),
                    })?;
                    note_session_guid(attempt_session_guid, &execution_result.metadata).map_err(|error| AttemptFailure {
                        emitted_error: structured_error.is_some(),
                        error,
                        output_manifest_committed: result.is_some(),
                        phase: Some(run.phase.unwrap_or(RunPhase::ExecuteRun)),
                        retriable: matches!(run.phase, Some(RunPhase::InstallPackages)),
                    })?;
                    run.last_session_guid = attempt_session_guid.clone();
                    self.emit_status(
                        run,
                        Some(active),
                        RunPhase::ExecuteRun,
                        "completed",
                        Some(execution_result.metadata.clone()),
                    ).await.map_err(store_failure)?;
                    execution = Some(execution_result);
                }
            }
        }
    }

    async fn upload_session_artifacts(
        &self,
        scope: &Scope,
        input_path: &str,
        input_artifact: (String, Vec<u8>),
        runtime: &SessionRuntimeArtifacts,
        attempt_session_guid: &mut Option<String>,
    ) -> Result<(), AppError> {
        let (input_content_type, input_bytes) = input_artifact;
        let input_upload = self
            .session_service
            .upload_session_file_detailed(scope, input_path, Some(input_content_type), input_bytes)
            .await?;
        note_session_guid(attempt_session_guid, &input_upload.metadata)?;

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

    async fn finish_success(
        &self,
        scope: &Scope,
        run_id: &str,
        attempt: i32,
        _input_path: &str,
        success: AttemptSuccess,
        active: Option<&ActiveRunHandle>,
    ) -> Result<RunResponse, AppError> {
        let mut run = self.load_run(scope, run_id).await?;
        run.attempt_count = attempt;
        run.status = RunStatus::Running;
        run.phase = Some(RunPhase::PersistOutputs);
        run.validation_issues = success.validation_issues.clone();
        run.output_path = Some(success.output_path.clone());
        self.run_store.save_run(&run).await?;

        self.emit_status(&mut run, active, RunPhase::PersistOutputs, "started", None)
            .await?;

        let download = self
            .session_service
            .download_session_file_detailed(scope, &success.output_path)
            .await?;
        let mut session_guid = run.last_session_guid.clone();
        note_session_guid(&mut session_guid, &download.metadata)?;
        run.last_session_guid = session_guid;

        self.artifact_store
            .upload(
                scope,
                &success.output_path,
                Some(download.value.0.as_str()),
                download.value.1,
            )
            .await?;

        self.emit_status(
            &mut run,
            active,
            RunPhase::PersistOutputs,
            "completed",
            Some(download.metadata),
        )
        .await?;

        run.status = RunStatus::Succeeded;
        run.phase = Some(RunPhase::PersistOutputs);
        run.output_path = Some(success.output_path.clone());
        run.validation_issues = success.validation_issues.clone();
        run.error_message = None;
        self.run_store.save_run(&run).await?;
        self.emit_event(
            run_id,
            active,
            RunEventPayload::Complete {
                final_status: RunStatus::Succeeded,
            },
        )
        .await?;

        Ok(RunResponse {
            run_id: run_id.to_string(),
            output_path: success.output_path,
            validation_issues: success.validation_issues,
        })
    }

    async fn finish_failure(
        &self,
        scope: &Scope,
        run_id: &str,
        attempt: i32,
        input_path: &str,
        failure: AttemptFailure,
        active: Option<&ActiveRunHandle>,
    ) -> Result<RunResponse, AppError> {
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

    async fn finish_cancelled(
        &self,
        scope: &Scope,
        run_id: &str,
        active: Option<&ActiveRunHandle>,
    ) -> Result<RunResponse, AppError> {
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

    async fn handle_runtime_event(
        &self,
        run_id: &str,
        run: &mut StoredRun,
        active: Option<&ActiveRunHandle>,
        event: RunEventPayload,
    ) -> Result<(), AppError> {
        match &event {
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
            RunEventPayload::Result {
                output_path,
                validation_issues,
            } => {
                run.phase = Some(RunPhase::ExecuteRun);
                run.output_path = Some(output_path.clone());
                run.validation_issues = validation_issues.clone();
            }
            RunEventPayload::Error { phase, message, .. } => {
                run.phase = *phase;
                run.error_message = Some(message.clone());
            }
            RunEventPayload::Log { phase, .. } => {
                run.phase = Some(*phase);
            }
            RunEventPayload::Complete { final_status } => {
                run.status = *final_status;
            }
        }

        self.run_store.save_run(run).await?;
        self.emit_event(run_id, active, event).await?;
        Ok(())
    }

    async fn emit_status(
        &self,
        run: &mut StoredRun,
        active: Option<&ActiveRunHandle>,
        phase: RunPhase,
        state: &str,
        metadata: Option<SessionOperationMetadata>,
    ) -> Result<(), AppError> {
        let payload = RunEventPayload::Status {
            phase,
            state: state.to_string(),
            session_guid: metadata
                .as_ref()
                .and_then(|meta| meta.session_guid.clone())
                .or_else(|| run.last_session_guid.clone()),
            operation_id: metadata.as_ref().and_then(|meta| meta.operation_id.clone()),
            timings: metadata.and_then(|meta| meta.timings),
        };
        self.handle_runtime_event(&run.run_id.clone(), run, active, payload)
            .await
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

    async fn load_run(&self, scope: &Scope, run_id: &str) -> Result<StoredRun, AppError> {
        self.run_store
            .get_run(scope, run_id)
            .await?
            .ok_or_else(|| AppError::not_found("Run not found."))
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
                ));
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

    async fn wait_for_run_bridge(
        &self,
        pending: PendingRunBridge,
        active: &ActiveRunHandle,
        _run_id: &str,
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
}

fn uploaded_filename(filename: &str) -> Result<String, AppError> {
    let name = Path::new(filename.trim())
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

fn session_path(relative_path: &str) -> String {
    format!("/mnt/data/{}", relative_path.trim_start_matches('/'))
}

fn session_output_dir(run_id: &str) -> String {
    session_path(&format!("runs/{run_id}/output"))
}

fn store_failure(error: AppError) -> AttemptFailure {
    AttemptFailure {
        emitted_error: false,
        error,
        output_manifest_committed: false,
        phase: None,
        retriable: false,
    }
}

fn cancelled_failure() -> AttemptFailure {
    AttemptFailure {
        emitted_error: true,
        error: AppError::request("Run cancelled."),
        output_manifest_committed: false,
        phase: None,
        retriable: false,
    }
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

fn parse_browser_message(text: &str) -> Result<BrowserRunClientMessage, AppError> {
    serde_json::from_str::<BrowserRunClientMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid run websocket message: {error}")))
}

fn parse_bridge_message(text: &str) -> Result<RunBridgeClientMessage, AppError> {
    serde_json::from_str::<RunBridgeClientMessage>(text)
        .map_err(|error| AppError::request(format!("Invalid run bridge message: {error}")))
}

async fn read_attach_message(socket: &mut WebSocket) -> Result<AttachMessage, AppError> {
    loop {
        match socket.recv().await {
            Some(Ok(Message::Text(text))) => match parse_browser_message(text.as_str())? {
                BrowserRunClientMessage::Attach { last_seen_seq } => {
                    return Ok(AttachMessage { last_seen_seq });
                }
                BrowserRunClientMessage::Cancel => {
                    return Err(AppError::request(
                        "The run events websocket must begin with an attach message.",
                    ));
                }
            },
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => {
                return Err(AppError::request(
                    "The run events websocket closed before attach.",
                ));
            }
            Some(Ok(Message::Binary(_))) => {
                return Err(AppError::request(
                    "Binary run websocket messages are not supported.",
                ));
            }
            Some(Err(error)) => {
                return Err(AppError::internal_with_source(
                    "Failed to read the run events websocket.",
                    error,
                ));
            }
        }
    }
}

struct AttachMessage {
    last_seen_seq: Option<i64>,
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

fn run_events_path(workspace_id: &str, config_version_id: &str, run_id: &str) -> String {
    format!("/api/workspaces/{workspace_id}/configs/{config_version_id}/runs/{run_id}/events")
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

#[derive(Debug)]
struct ParsedSyncOutput {
    events: Vec<RunEventPayload>,
    result: Option<AttemptSuccess>,
}

fn parse_sync_output(stdout: &str) -> Result<ParsedSyncOutput, AppError> {
    let mut events = Vec::new();
    let mut result = None;

    for line in stdout.lines() {
        let Some(payload) = line.strip_prefix(RUN_EVENT_SENTINEL_PREFIX) else {
            continue;
        };

        let event = serde_json::from_str::<RunEventPayload>(payload).map_err(|error| {
            AppError::internal_with_source("Failed to decode a structured run event.", error)
        })?;

        if let RunEventPayload::Result {
            output_path,
            validation_issues,
        } = &event
        {
            result = Some(AttemptSuccess {
                output_path: output_path.clone(),
                validation_issues: validation_issues.clone(),
            });
        }

        events.push(event);
    }

    Ok(ParsedSyncOutput { events, result })
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
    use super::{
        RUN_EVENT_SENTINEL_PREFIX, parse_sync_output, render_bootstrap_code, run_events_path,
        uploaded_filename,
    };
    use crate::runs::RunBootstrapConfig;

    #[test]
    fn sync_output_parser_extracts_structured_events() {
        let parsed = parse_sync_output(&format!(
            "noise\n{RUN_EVENT_SENTINEL_PREFIX}{{\"type\":\"status\",\"phase\":\"uploadArtifacts\",\"state\":\"started\"}}\n{RUN_EVENT_SENTINEL_PREFIX}{{\"type\":\"result\",\"outputPath\":\"runs/run-1/output/out.xlsx\",\"validationIssues\":[]}}\n"
        ))
        .unwrap();

        assert_eq!(parsed.events.len(), 2);
        assert_eq!(
            parsed.result.expect("result").output_path,
            "runs/run-1/output/out.xlsx"
        );
    }

    #[test]
    fn bootstrap_code_contains_required_fields() {
        let code = render_bootstrap_code(&RunBootstrapConfig {
            bridge_url: Some("wss://example.com/bridge".to_string()),
            config_package_name: "ade-config",
            config_version: "1.0.0".to_string(),
            config_wheel_path: "/mnt/data/config.whl".to_string(),
            engine_package_name: "ade-engine",
            engine_version: "1.0.0".to_string(),
            engine_wheel_path: "/mnt/data/engine.whl".to_string(),
            event_sentinel_prefix: None,
            input_path: "/mnt/data/input.xlsx".to_string(),
            install_lock_path: "/mnt/data/.lock".to_string(),
            output_dir: "/mnt/data/runs/run-1/output".to_string(),
        })
        .unwrap();

        assert!(code.contains("websockets.sync.client"));
        assert!(code.contains("installPackages"));
        assert!(code.contains("/mnt/data/runs/run-1/output"));
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
