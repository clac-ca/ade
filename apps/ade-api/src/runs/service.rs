mod execution;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::ws::{Message, WebSocket},
    http::StatusCode,
};
use reqwest::Url;
use tokio::{
    sync::{broadcast, oneshot, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    artifacts::{
        ArtifactStoreHandle, artifact_store_from_env, output_path_for_run, upload_id,
        upload_path_for_file, validate_input_path, verify_local_artifact_access,
    },
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
    session::{
        PythonExecution, SessionOperationMetadata, SessionOperationResult, SessionRuntimeArtifacts,
        SessionService,
    },
    unix_time_ms,
};

use super::{
    bootstrap::{
        RunBootstrapConfig, bootstrap_artifact_access, local_input_path, local_output_dir,
        render_bootstrap_code, session_path,
    },
    bridge::{
        PendingRunBridgeManager, RunBridgeClientMessage, RunBridgeServerMessage,
        create_bridge_token, parse_bridge_message, verify_bridge_token,
    },
    events::{RunEventFeed, run_events_path},
    models::{
        AsyncRunResponse, CreateRunRequest, CreateUploadRequest, CreateUploadResponse,
        RunDetailResponse, RunValidationIssue, UploadInstruction,
    },
    store::{RunEvent, RunEventPayload, RunPhase, RunStatus, RunStore, RunStoreHandle, StoredRun},
};

const APP_URL_ENV_NAME: &str = "ADE_APP_URL";
const BRIDGE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const BRIDGE_TOKEN_TTL_MS: u64 = 60_000;
const RUN_ACCESS_TTL_SECONDS: u64 = 900;
const RUN_EXECUTION_TIMEOUT_SECONDS: u64 = 900;
const RUN_MAX_ATTEMPTS: i32 = 2;

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
}

impl Drop for ActiveRunHandle {
    fn drop(&mut self) {
        self.manager.remove(&self.run_id);
    }
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
            artifact_store: artifact_store_from_env(env, session_service.session_secret())?,
            bridge_manager: PendingRunBridgeManager::default(),
            run_store,
            session_secret: session_service.session_secret().to_string(),
            session_service,
        })
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
            events_url: run_events_path(&scope.workspace_id, &scope.config_version_id, &run_id),
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
        let expires_at = time::OffsetDateTime::now_utc()
            + time::Duration::seconds(RUN_ACCESS_TTL_SECONDS as i64);
        let upload = self
            .artifact_store
            .create_browser_upload_access(&file_path, request.content_type.as_deref(), expires_at)
            .await?;

        Ok(CreateUploadResponse {
            file_path,
            upload: UploadInstruction {
                expires_at: upload.expires_at,
                headers: upload.headers,
                method: upload.method.to_string(),
                url: upload.url,
            },
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

    async fn emit_event(
        &self,
        run_id: &str,
        active: Option<&ActiveRunHandle>,
        event: RunEventPayload,
    ) -> Result<RunEvent, AppError> {
        let event = self.run_store.append_event(run_id, event).await?;
        if let Some(active) = active {
            let _ = active.sender.send(event.clone());
        }
        Ok(event)
    }
}
