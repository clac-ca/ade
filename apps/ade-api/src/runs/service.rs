mod execution;

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::http::StatusCode;
use sha2::{Digest, Sha256};
use tokio::{
    io::AsyncWriteExt,
    sync::{Semaphore, broadcast, watch},
};
use tracing::error;
use uuid::Uuid;

use crate::{
    artifacts::{
        BlobArtifactStore, blob_artifact_store_from_env, log_path_for_run, output_path_for_run,
        upload_batch_id, upload_file_id, upload_id, upload_path_for_batch_file,
        upload_path_for_file, validate_input_path,
    },
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    sandbox_environment::SandboxEnvironmentManager,
    scope::Scope,
};

use super::{
    events::{RunEventFeed, archive_event_line, parse_archived_events},
    models::{
        AsyncRunResponse, CreateDownloadRequest, CreateDownloadResponse, CreateRunRequest,
        CreateUploadBatchRequest, CreateUploadBatchResponse, CreateUploadRequest,
        CreateUploadResponse, RunArtifactKind, RunDetailResponse, RunValidationIssue,
    },
    store::{RunEvent, RunEventPayload, RunPhase, RunStatus, RunStore, RunStoreHandle, StoredRun},
};

const BULK_UPLOAD_ACCESS_TTL_SECONDS: u64 = 1_800;
const BULK_UPLOAD_MAX_FILE_COUNT: usize = 100;
const BULK_UPLOAD_MAX_TOTAL_SIZE_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const RUN_ACCESS_TTL_SECONDS: u64 = 900;
const RUN_TIMEOUT_DEFAULT_SECONDS: u64 = 900;
const RUN_MAX_CONCURRENT_DEFAULT: usize = 4;
const RUN_MAX_CONCURRENT_ENV_NAME: &str = "ADE_RUN_MAX_CONCURRENT";
const RUN_MAX_ATTEMPTS: i32 = 2;
const RUN_LOG_CONTENT_TYPE: &str = "application/x-ndjson";
const RUN_REPLAY_WINDOW_SIZE: usize = 2_048;
#[derive(Clone)]
pub struct RunService {
    active_runs: ActiveRunManager,
    artifact_store: Arc<BlobArtifactStore>,
    run_store: RunStoreHandle,
    permits: Arc<Semaphore>,
    sandbox_environment_manager: Arc<SandboxEnvironmentManager>,
}

#[derive(Clone)]
struct ActiveRunState {
    broadcaster: broadcast::Sender<RunEvent>,
    cancel_tx: watch::Sender<bool>,
    events: Arc<tokio::sync::Mutex<ActiveRunBuffer>>,
}

struct ActiveRunBuffer {
    log_spool_failed: bool,
    next_seq: i64,
    replay: VecDeque<RunEvent>,
    spool_path: PathBuf,
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
            events: Arc::new(tokio::sync::Mutex::new(ActiveRunBuffer {
                log_spool_failed: false,
                next_seq: 1,
                replay: VecDeque::with_capacity(RUN_REPLAY_WINDOW_SIZE),
                spool_path: run_log_spool_path(run_id),
            })),
        };

        self.inner
            .lock()
            .expect("active run lock poisoned")
            .insert(run_id.to_string(), state.clone());

        ActiveRunHandle {
            cancel_rx,
            manager: self.clone(),
            run_id: run_id.to_string(),
            state,
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

    fn get(&self, run_id: &str) -> Option<ActiveRunState> {
        self.inner
            .lock()
            .expect("active run lock poisoned")
            .get(run_id)
            .cloned()
    }
}

struct ActiveRunHandle {
    cancel_rx: watch::Receiver<bool>,
    manager: ActiveRunManager,
    run_id: String,
    state: ActiveRunState,
}

impl Drop for ActiveRunHandle {
    fn drop(&mut self) {
        self.manager
            .inner
            .lock()
            .expect("active run lock poisoned")
            .remove(&self.run_id);
    }
}

impl ActiveRunState {
    async fn archive_log(
        &self,
        run_id: &str,
        artifact_store: &BlobArtifactStore,
        log_path: &str,
    ) -> Result<(), AppError> {
        let spool_path = {
            let events = self.events.lock().await;
            if events.log_spool_failed {
                return Err(AppError::internal(format!(
                    "Run log spool failed for '{run_id}'."
                )));
            }
            events.spool_path.clone()
        };

        artifact_store
            .upload_file(log_path, Some(RUN_LOG_CONTENT_TYPE), &spool_path)
            .await?;
        if let Err(error) = tokio::fs::remove_file(&spool_path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            error!(
                run_id,
                path = %spool_path.display(),
                error = %error,
                "Failed to remove the run log spool file."
            );
        }
        Ok(())
    }

    async fn emit_event(&self, run_id: &str, event: RunEventPayload) -> RunEvent {
        let event = {
            let mut events = self.events.lock().await;
            let event = event.with_seq(events.next_seq);
            events.next_seq += 1;
            events.replay.push_back(event.clone());
            while events.replay.len() > RUN_REPLAY_WINDOW_SIZE {
                events.replay.pop_front();
            }

            if !events.log_spool_failed {
                match append_run_log_line(&events.spool_path, run_id, &event).await {
                    Ok(()) => {}
                    Err(error) => {
                        events.log_spool_failed = true;
                        error!(
                            run_id,
                            path = %events.spool_path.display(),
                            error = %error,
                            "Failed to append a run event to the NDJSON spool."
                        );
                    }
                }
            }

            event
        };

        let _ = self.broadcaster.send(event.clone());
        event
    }

    async fn replay_events(&self, after_seq: Option<i64>) -> Result<Vec<RunEvent>, AppError> {
        let events = self.events.lock().await;
        replay_run_events(&events.replay, after_seq)
    }

    fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.broadcaster.subscribe()
    }
}

async fn append_run_log_line(
    spool_path: &PathBuf,
    run_id: &str,
    event: &RunEvent,
) -> Result<(), AppError> {
    if let Some(parent) = spool_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            AppError::io_with_source(format!("Failed to create '{}'.", parent.display()), error)
        })?;
    }

    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(spool_path)
        .await
        .map_err(|error| {
            AppError::io_with_source(format!("Failed to open '{}'.", spool_path.display()), error)
        })?;
    let line = format!("{}\n", archive_event_line(run_id, event)?);
    file.write_all(line.as_bytes()).await.map_err(|error| {
        AppError::io_with_source(
            format!("Failed to write '{}'.", spool_path.display()),
            error,
        )
    })?;
    Ok(())
}

fn replay_run_events(
    replay: &VecDeque<RunEvent>,
    after_seq: Option<i64>,
) -> Result<Vec<RunEvent>, AppError> {
    let Some(after_seq) = after_seq else {
        return Ok(replay.iter().cloned().collect());
    };

    if let Some(oldest_seq) = replay.front().map(RunEvent::seq)
        && after_seq < oldest_seq.saturating_sub(1)
    {
        return Err(AppError::status(
            StatusCode::CONFLICT,
            "Run event replay window expired. Use the archived log artifact for full history.",
        ));
    }

    Ok(replay
        .iter()
        .filter(|event| event.seq() > after_seq)
        .cloned()
        .collect())
}

fn append_terminal_complete_event(replay: &mut Vec<RunEvent>, run: &StoredRun) {
    if replay
        .iter()
        .any(|event| matches!(event, RunEvent::Complete { .. }))
    {
        return;
    }

    replay.push(RunEvent::Complete {
        seq: replay.last().map(RunEvent::seq).unwrap_or(0) + 1,
        final_status: run.status,
        log_path: run.log_path.clone(),
        output_path: run.output_path.clone(),
    });
}

fn run_log_spool_path(run_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("ade-run-logs")
        .join(format!("{run_id}-{}.ndjson", Uuid::new_v4().simple()))
}

fn run_id_for_input(scope: &Scope, input_path: &str) -> String {
    let digest = Sha256::digest(format!(
        "run:{}:{}:{}",
        scope.workspace_id, scope.config_version_id, input_path
    ));
    format!("run_{}", hex::encode(&digest[..16]))
}

impl RunService {
    pub fn from_env(
        env: &EnvBag,
        sandbox_environment_manager: Arc<SandboxEnvironmentManager>,
        run_store: Arc<dyn RunStore>,
    ) -> Result<Self, AppError> {
        Ok(Self {
            active_runs: ActiveRunManager::default(),
            artifact_store: Arc::new(blob_artifact_store_from_env(env)?),
            run_store,
            permits: Arc::new(Semaphore::new(read_run_max_concurrent(env)?)),
            sandbox_environment_manager,
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

        let run_id = run_id_for_input(&scope, &input_path);
        if let Some(existing) = self.run_store.get_run(&scope, &run_id).await? {
            return Ok(AsyncRunResponse {
                run_id: existing.run_id,
                status: existing.status,
            });
        }

        let output_path = output_path_for_run(&scope, &run_id);
        if let Err(error) = self
            .run_store
            .create_run(&scope, &run_id, &input_path)
            .await
        {
            if let Some(existing) = self.run_store.get_run(&scope, &run_id).await?
                && existing.input_path == input_path
            {
                return Ok(AsyncRunResponse {
                    run_id: existing.run_id,
                    status: existing.status,
                });
            }
            return Err(error);
        }
        let active = self.active_runs.register(&run_id);
        self.emit_event(
            &active,
            RunEventPayload::Created {
                status: RunStatus::Pending,
            },
        )
        .await;
        let service = self.clone();
        let run_id_for_task = run_id.clone();
        let run_id_for_log = run_id.clone();
        let input_path_for_task = input_path.clone();
        let output_path_for_task = output_path.clone();
        let scope_for_task = scope.clone();
        tokio::spawn(async move {
            match service
                .drive_run(
                    scope_for_task,
                    run_id_for_task,
                    input_path_for_task,
                    output_path_for_task,
                    request.timeout_in_seconds,
                    active,
                )
                .await
            {
                Err(error) if !matches!(&error, AppError::Request(message) if message == "Run cancelled.") =>
                {
                    error!(run_id = %run_id_for_log, error = %error, "Run task exited with an error.");
                }
                _ => {}
            }
        });

        Ok(AsyncRunResponse {
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
        let file_path = upload_path_for_file(scope, &upload_id, &request.filename)?;
        let expires_at = time::OffsetDateTime::now_utc()
            + time::Duration::seconds(RUN_ACCESS_TTL_SECONDS as i64);
        let upload = self
            .artifact_store
            .create_browser_upload_access(&file_path, request.content_type.as_deref(), expires_at)
            .await?;

        Ok(CreateUploadResponse {
            file_path,
            upload: upload.into(),
        })
    }

    pub(crate) async fn create_upload_batch(
        &self,
        scope: &Scope,
        request: CreateUploadBatchRequest,
    ) -> Result<CreateUploadBatchResponse, AppError> {
        validate_upload_batch(&request)?;

        let batch_id = upload_batch_id();
        let expires_at = time::OffsetDateTime::now_utc()
            + time::Duration::seconds(BULK_UPLOAD_ACCESS_TTL_SECONDS as i64);
        let mut items = Vec::with_capacity(request.files.len());
        for file in request.files {
            let file_id = upload_file_id();
            let file_path = upload_path_for_batch_file(scope, &batch_id, &file_id, &file.filename)?;
            let upload = self
                .artifact_store
                .create_browser_upload_access(&file_path, file.content_type.as_deref(), expires_at)
                .await?;
            items.push(super::models::CreateUploadBatchItem {
                file_id,
                file_path,
                upload: upload.into(),
            });
        }

        Ok(CreateUploadBatchResponse { batch_id, items })
    }

    pub(crate) async fn create_download(
        &self,
        scope: &Scope,
        run_id: &str,
        request: CreateDownloadRequest,
    ) -> Result<CreateDownloadResponse, AppError> {
        let run = self
            .run_store
            .get_run(scope, run_id)
            .await?
            .ok_or_else(|| AppError::not_found("Run not found."))?;
        let file_path = match request.artifact {
            RunArtifactKind::Log => run
                .log_path
                .clone()
                .ok_or_else(|| AppError::status(StatusCode::CONFLICT, "Run log is not ready."))?,
            RunArtifactKind::Output => run.output_path.clone().ok_or_else(|| {
                AppError::status(StatusCode::CONFLICT, "Run output is not ready.")
            })?,
        };
        let expires_at = time::OffsetDateTime::now_utc()
            + time::Duration::seconds(RUN_ACCESS_TTL_SECONDS as i64);
        let download = self
            .artifact_store
            .create_browser_download_access(&file_path, expires_at)
            .await?;

        Ok(CreateDownloadResponse {
            download: download.into(),
            file_path,
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
            log_path: run.log_path.clone(),
            output_path: run.output_path.clone(),
            phase: run.phase,
            run_id: run.run_id.clone(),
            status: run.status,
            validation_issues: run.validation_issues.clone(),
        })
    }

    pub(crate) async fn cancel_run(&self, scope: &Scope, run_id: &str) -> Result<(), AppError> {
        let Some(run) = self.run_store.get_run(scope, run_id).await? else {
            return Err(AppError::not_found("Run not found."));
        };

        if run.status.is_terminal() {
            return Ok(());
        }

        if self.active_runs.cancel(run_id) {
            return Ok(());
        }

        Err(AppError::status(
            StatusCode::CONFLICT,
            "Run is not active on this API instance and cannot be cancelled.",
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

        if run.status.is_terminal() {
            if let Some(log_path) = run.log_path.as_deref() {
                let log_bytes = self.artifact_store.download_bytes(log_path).await?;
                let log_text = String::from_utf8(log_bytes).map_err(|error| {
                    AppError::internal_with_source("Failed to decode the archived run log.", error)
                })?;
                let mut replay = parse_archived_events(&log_text)?;
                append_terminal_complete_event(&mut replay, &run);
                return Ok(RunEventFeed {
                    replay: replay_run_events(&VecDeque::from(replay), after_seq)?,
                    subscription: None,
                });
            }

            if let Some(active) = self.active_runs.get(run_id) {
                return Ok(RunEventFeed {
                    replay: active.replay_events(after_seq).await?,
                    subscription: None,
                });
            }

            return Ok(RunEventFeed {
                replay: Vec::new(),
                subscription: None,
            });
        }

        let Some(active) = self.active_runs.get(run_id) else {
            return Err(AppError::status(
                StatusCode::CONFLICT,
                "Run event stream is unavailable because the run is not active on this API instance.",
            ));
        };

        Ok(RunEventFeed {
            replay: active.replay_events(after_seq).await?,
            subscription: Some(active.subscribe()),
        })
    }
    async fn drive_run(
        &self,
        scope: Scope,
        run_id: String,
        input_path: String,
        output_path: String,
        timeout_in_seconds: Option<u64>,
        active: ActiveRunHandle,
    ) -> Result<(), AppError> {
        let mut cancel_rx = active.cancel_rx.clone();
        let _permit = match tokio::select! {
            permit = Arc::clone(&self.permits).acquire_owned() => permit,
            _ = cancel_rx.changed() => return self.finish_cancelled(&scope, &run_id, &active).await,
        } {
            Ok(permit) => permit,
            Err(_) => {
                return self
                    .finish_failure(
                        &scope,
                        &run_id,
                        0,
                        &input_path,
                        execution::attempt_failure(
                            AppError::internal("Run scheduler is unavailable."),
                            Some(RunPhase::Execute),
                        ),
                        &active,
                    )
                    .await;
            }
        };

        self.execute_run(
            scope,
            run_id,
            input_path,
            output_path,
            timeout_in_seconds,
            active,
        )
        .await
    }

    async fn finalize_run_log(&self, scope: &Scope, run: &mut StoredRun, active: &ActiveRunHandle) {
        let log_path = log_path_for_run(scope, &run.run_id);

        match active
            .state
            .archive_log(&active.run_id, self.artifact_store.as_ref(), &log_path)
            .await
        {
            Ok(()) => {
                run.log_path = Some(log_path);
            }
            Err(error) => {
                error!(
                    run_id = %run.run_id,
                    error = %error,
                    "Failed to archive the run log."
                );
            }
        }
    }

    async fn emit_event(&self, active: &ActiveRunHandle, event: RunEventPayload) {
        active.state.emit_event(&active.run_id, event).await;
    }
}

fn read_run_max_concurrent(env: &EnvBag) -> Result<usize, AppError> {
    let Some(value) = read_optional_trimmed_string(env, RUN_MAX_CONCURRENT_ENV_NAME) else {
        return Ok(RUN_MAX_CONCURRENT_DEFAULT);
    };
    let parsed = value.parse::<usize>().map_err(|error| {
        AppError::config_with_source(
            format!("{RUN_MAX_CONCURRENT_ENV_NAME} must be a positive integer."),
            error,
        )
    })?;
    if parsed == 0 {
        return Err(AppError::config(format!(
            "{RUN_MAX_CONCURRENT_ENV_NAME} must be greater than zero."
        )));
    }
    Ok(parsed)
}

fn validate_upload_batch(request: &CreateUploadBatchRequest) -> Result<(), AppError> {
    if request.files.is_empty() || request.files.len() > BULK_UPLOAD_MAX_FILE_COUNT {
        return Err(AppError::request(format!(
            "Bulk upload batches must include between 1 and {BULK_UPLOAD_MAX_FILE_COUNT} files."
        )));
    }

    let total_size = request.files.iter().try_fold(0_u64, |total, file| {
        if file.size == 0 {
            return Err(AppError::request(
                "Bulk upload files must declare a size greater than zero.",
            ));
        }
        total.checked_add(file.size).ok_or_else(|| {
            AppError::request("Bulk upload batch size exceeded the supported limit.")
        })
    })?;

    if total_size > BULK_UPLOAD_MAX_TOTAL_SIZE_BYTES {
        return Err(AppError::request(
            "Bulk upload batches must not exceed 5 GiB in declared size.",
        ));
    }

    Ok(())
}

fn run_timeout_in_seconds(timeout_in_seconds: Option<u64>) -> u64 {
    timeout_in_seconds.unwrap_or(RUN_TIMEOUT_DEFAULT_SECONDS)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use axum::http::StatusCode;

    use super::{RUN_REPLAY_WINDOW_SIZE, replay_run_events};
    use crate::runs::store::{RunEvent, RunPhase, RunStatus};

    #[test]
    fn replay_buffer_rejects_requests_older_than_the_oldest_available_event() {
        let replay = (2..=RUN_REPLAY_WINDOW_SIZE as i64 + 1)
            .map(|seq| RunEvent::Log {
                seq,
                level: "info".to_string(),
                message: format!("event {seq}"),
                phase: RunPhase::Execute,
            })
            .collect::<VecDeque<_>>();

        let error = replay_run_events(&replay, Some(0)).unwrap_err();
        match error {
            crate::error::AppError::Response { status, .. } => {
                assert_eq!(status, StatusCode::CONFLICT);
            }
            other => panic!("expected conflict response, got {other}"),
        }
    }

    #[test]
    fn replay_buffer_returns_events_after_the_requested_sequence() {
        let replay = VecDeque::from([
            RunEvent::Created {
                seq: 10,
                status: RunStatus::Pending,
            },
            RunEvent::Status {
                seq: 11,
                phase: RunPhase::Execute,
                state: "started".to_string(),
                session_guid: None,
                operation_id: None,
                timings: None,
            },
            RunEvent::Complete {
                seq: 12,
                final_status: RunStatus::Succeeded,
                log_path: None,
                output_path: None,
            },
        ]);

        let replayed = replay_run_events(&replay, Some(10)).unwrap();
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].seq(), 11);
        assert_eq!(replayed[1].seq(), 12);
    }
}
