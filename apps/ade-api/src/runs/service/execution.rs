use std::{collections::BTreeMap, time::Duration};

use super::*;

use crate::{
    artifacts::ArtifactAccessGrant,
    runs::models::ArtifactAccessInstruction,
    scope_session::{
        ChannelId, ChannelKind, ChannelOpenParams, ChannelStream, ScopeSessionEvent, SignalName,
    },
};

const PROCESS_RESULT_PREFIX: &str = "__ADE_PROCESS_RESULT__=";
const RUN_SCRIPT: &str = r#"exec "$ADE_PYTHON_BIN" - <<'PY'
import json
import os
import shutil
from pathlib import Path
from urllib.request import Request, urlopen

from ade_engine import load_config
from ade_engine.runner import process

PROCESS_RESULT_PREFIX = "__ADE_PROCESS_RESULT__="

def access(name: str) -> dict:
    return json.loads(os.environ[name])

def download(blob: dict, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    request = Request(blob["url"], headers=blob["headers"], method=blob["method"])
    with urlopen(request) as response, destination.open("wb") as handle:
        shutil.copyfileobj(response, handle)

def upload(blob: dict, source: Path) -> None:
    request = Request(
        blob["url"],
        data=source.read_bytes(),
        headers=blob["headers"],
        method=blob["method"],
    )
    with urlopen(request) as response:
        response.read()

input_path = Path(os.environ["ADE_LOCAL_INPUT_PATH"])
output_dir = Path(os.environ["ADE_LOCAL_OUTPUT_DIR"])
print(f"Downloading input to {input_path}", flush=True)
download(access("ADE_INPUT_ACCESS_JSON"), input_path)

print(f"Processing workbook into {output_dir}", flush=True)
config = load_config("ade_config", name="ade-config")
result = process(config=config, input_path=input_path, output_dir=output_dir)

print(f"Uploading output from {result.output_path}", flush=True)
upload(access("ADE_OUTPUT_ACCESS_JSON"), result.output_path)
print(
    PROCESS_RESULT_PREFIX
    + json.dumps(
        {
            "outputPath": os.environ["ADE_OUTPUT_PATH"],
            "validationIssues": [
                {
                    "rowIndex": issue.row_index,
                    "field": issue.field,
                    "message": issue.message,
                }
                for issue in result.validation_issues
            ],
        },
        separators=(",", ":"),
    ),
    flush=True,
)
PY
"#;

#[derive(Clone, Debug)]
struct AttemptSuccess {
    output_path: String,
    validation_issues: Vec<RunValidationIssue>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessResult {
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
pub(super) struct AttemptFailure {
    emitted_error: bool,
    error: AppError,
    phase: Option<RunPhase>,
    retriable: bool,
    status: RunStatus,
}

impl RunService {
    pub(super) async fn execute_run(
        &self,
        scope: Scope,
        run_id: String,
        input_path: String,
        output_path: String,
        timeout_in_seconds: Option<u64>,
        active: ActiveRunHandle,
    ) -> Result<(), AppError> {
        for attempt in 1..=RUN_MAX_ATTEMPTS {
            if *active.cancel_rx.borrow() {
                return self.finish_cancelled(&scope, &run_id, &active).await;
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
                    if *active.cancel_rx.borrow() {
                        return self.finish_cancelled(&scope, &run_id, &active).await;
                    }
                    return self
                        .finish_success(&scope, &run_id, attempt, success, &active)
                        .await;
                }
                Err(failure) => failure,
            };

            if *active.cancel_rx.borrow() {
                return self.finish_cancelled(&scope, &run_id, &active).await;
            }

            if !(failure.retriable && attempt < RUN_MAX_ATTEMPTS) {
                return self
                    .finish_failure(&scope, &run_id, attempt, &input_path, failure, &active)
                    .await;
            }
        }

        Err(AppError::status(StatusCode::BAD_GATEWAY, "ADE run failed."))
    }

    pub(super) async fn finish_cancelled(
        &self,
        scope: &Scope,
        run_id: &str,
        active: &ActiveRunHandle,
    ) -> Result<(), AppError> {
        let mut run = self.load_run(scope, run_id).await?;
        run.status = RunStatus::Cancelled;
        run.error_message = Some("Run cancelled.".to_string());
        run.log_path = None;
        run.output_path = None;
        run.validation_issues.clear();
        self.finalize_run_log(scope, &mut run, active).await;
        self.run_store.save_run(&run).await?;
        self.emit_event(
            active,
            RunEventPayload::Complete {
                final_status: RunStatus::Cancelled,
                log_path: run.log_path.clone(),
                output_path: None,
            },
        )
        .await;
        Err(AppError::request("Run cancelled."))
    }

    pub(super) async fn finish_failure(
        &self,
        scope: &Scope,
        run_id: &str,
        attempt: i32,
        input_path: &str,
        failure: AttemptFailure,
        active: &ActiveRunHandle,
    ) -> Result<(), AppError> {
        let mut run = self.load_run(scope, run_id).await?;
        run.attempt_count = attempt;
        run.input_path = input_path.to_string();
        run.phase = failure.phase;
        run.status = failure.status;
        run.error_message = Some(failure.error.to_string());
        run.log_path = None;
        run.output_path = None;
        run.validation_issues.clear();

        if !failure.emitted_error {
            self.emit_event(
                active,
                RunEventPayload::Error {
                    phase: failure.phase,
                    message: failure.error.to_string(),
                    retriable: failure.retriable,
                },
            )
            .await;
        }

        self.finalize_run_log(scope, &mut run, active).await;
        self.run_store.save_run(&run).await?;
        self.emit_event(
            active,
            RunEventPayload::Complete {
                final_status: run.status,
                log_path: run.log_path.clone(),
                output_path: None,
            },
        )
        .await;
        Err(failure.error)
    }

    async fn finish_success(
        &self,
        scope: &Scope,
        run_id: &str,
        attempt: i32,
        success: AttemptSuccess,
        active: &ActiveRunHandle,
    ) -> Result<(), AppError> {
        let mut run = self.load_run(scope, run_id).await?;
        run.attempt_count = attempt;
        run.error_message = None;
        run.log_path = None;
        run.output_path = Some(success.output_path.clone());
        run.status = RunStatus::Succeeded;
        run.validation_issues = success.validation_issues.clone();
        self.finalize_run_log(scope, &mut run, active).await;
        self.run_store.save_run(&run).await?;
        self.emit_event(
            active,
            RunEventPayload::Complete {
                final_status: RunStatus::Succeeded,
                log_path: run.log_path.clone(),
                output_path: run.output_path.clone(),
            },
        )
        .await;
        Ok(())
    }

    async fn handle_runtime_event(
        &self,
        run: &mut StoredRun,
        active: &ActiveRunHandle,
        event: RunEventPayload,
    ) -> Result<(), AppError> {
        match &event {
            RunEventPayload::Created { .. } | RunEventPayload::Complete { .. } => {
                unreachable!("reverse-connect runtime does not emit created or completed events")
            }
            RunEventPayload::Status {
                phase,
                state,
                session_guid,
                ..
            } => {
                run.phase = Some(*phase);
                if matches!(state.as_str(), "started" | "completed") {
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
            RunEventPayload::Result { .. } => {
                run.phase = Some(RunPhase::PersistOutputs);
            }
        }

        self.run_store.save_run(run).await?;
        self.emit_event(active, event).await;
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
        let mut run = self
            .load_run(attempt.scope, attempt.run_id)
            .await
            .map_err(store_failure)?;
        run.attempt_count = attempt.attempt;
        run.error_message = None;
        run.log_path = None;
        run.output_path = None;
        run.phase = None;
        run.status = RunStatus::Running;
        run.validation_issues.clear();
        self.run_store.save_run(&run).await.map_err(store_failure)?;

        self.handle_runtime_event(
            &mut run,
            active,
            RunEventPayload::Status {
                phase: RunPhase::InstallPackages,
                state: "started".to_string(),
                session_guid: None,
                operation_id: None,
                timings: None,
            },
        )
        .await
        .map_err(store_failure)?;

        let handle = self
            .scope_session_service
            .ensure_ready_scope_session(attempt.scope)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::InstallPackages),
                retriable: true,
                status: RunStatus::Failed,
            })?;

        self.handle_runtime_event(
            &mut run,
            active,
            RunEventPayload::Status {
                phase: RunPhase::InstallPackages,
                state: "completed".to_string(),
                session_guid: None,
                operation_id: None,
                timings: None,
            },
        )
        .await
        .map_err(store_failure)?;

        if *active.cancel_rx.borrow() {
            return Err(AttemptFailure {
                emitted_error: true,
                error: AppError::request("Run cancelled."),
                phase: None,
                retriable: false,
                status: RunStatus::Cancelled,
            });
        }

        let access_expires_at = time::OffsetDateTime::now_utc()
            + time::Duration::seconds(run_access_ttl_seconds(attempt.timeout_in_seconds) as i64);
        let input_download = self
            .artifact_store
            .create_download_access(attempt.input_path, access_expires_at)
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::ExecuteRun),
                retriable: false,
                status: RunStatus::Failed,
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
                status: RunStatus::Failed,
            })?;

        let channel_id = ChannelId::new(format!(
            "run-{}-attempt-{}",
            attempt.run_id, attempt.attempt
        ));
        let mut events = handle.subscribe();
        handle
            .open_channel(ChannelOpenParams {
                channel_id: channel_id.clone(),
                command: RUN_SCRIPT.to_string(),
                cwd: Some(handle.session_root().to_string()),
                env: run_env(
                    handle.python_executable_path().to_string(),
                    resolve_artifact_access(&self.app_url, input_download).map_err(|error| {
                        AttemptFailure {
                            emitted_error: false,
                            error,
                            phase: Some(RunPhase::ExecuteRun),
                            retriable: false,
                            status: RunStatus::Failed,
                        }
                    })?,
                    resolve_artifact_access(&self.app_url, output_upload).map_err(|error| {
                        AttemptFailure {
                            emitted_error: false,
                            error,
                            phase: Some(RunPhase::PersistOutputs),
                            retriable: false,
                            status: RunStatus::Failed,
                        }
                    })?,
                    format!(
                        "/mnt/data/work/input/{}/attempt-{}/{}",
                        attempt.run_id,
                        attempt.attempt,
                        uploaded_basename(attempt.input_path)
                    ),
                    format!(
                        "/mnt/data/runs/{}/attempt-{}/output",
                        attempt.run_id, attempt.attempt
                    ),
                    attempt.output_path.to_string(),
                )
                .map_err(|error| AttemptFailure {
                    emitted_error: false,
                    error,
                    phase: Some(RunPhase::ExecuteRun),
                    retriable: false,
                    status: RunStatus::Failed,
                })?,
                kind: ChannelKind::Exec,
                pty: None,
            })
            .await
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::ExecuteRun),
                retriable: true,
                status: RunStatus::Failed,
            })?;

        self.handle_runtime_event(
            &mut run,
            active,
            RunEventPayload::Status {
                phase: RunPhase::ExecuteRun,
                state: "started".to_string(),
                session_guid: None,
                operation_id: None,
                timings: None,
            },
        )
        .await
        .map_err(store_failure)?;

        let mut cancel_rx = active.cancel_rx.clone();
        let timeout = tokio::time::sleep(Duration::from_secs(run_timeout_in_seconds(
            attempt.timeout_in_seconds,
        )));
        tokio::pin!(timeout);
        let mut stdout_lines = LineBuffer::default();
        let mut stderr_lines = LineBuffer::default();
        let mut success = None;

        loop {
            tokio::select! {
                _ = cancel_rx.changed() => {
                    let _ = handle.signal_channel(channel_id.clone(), SignalName::Kill).await;
                    let _ = handle.close_channel(channel_id.clone()).await;
                    return Err(AttemptFailure {
                        emitted_error: true,
                        error: AppError::request("Run cancelled."),
                        phase: None,
                        retriable: false,
                        status: RunStatus::Cancelled,
                    });
                }
                _ = &mut timeout => {
                    let _ = handle.signal_channel(channel_id.clone(), SignalName::Kill).await;
                    let _ = handle.close_channel(channel_id.clone()).await;
                    return Err(AttemptFailure {
                        emitted_error: false,
                        error: AppError::status(
                            StatusCode::BAD_GATEWAY,
                            format!(
                                "Run timed out after {} seconds.",
                                run_timeout_in_seconds(attempt.timeout_in_seconds)
                            ),
                        ),
                        phase: Some(RunPhase::ExecuteRun),
                        retriable: false,
                        status: RunStatus::Failed,
                    });
                }
                event = events.recv() => {
                    match event {
                        Ok(ScopeSessionEvent::Data { channel_id: event_channel_id, data, stream }) if event_channel_id == channel_id => {
                            match stream {
                                ChannelStream::Stdout => {
                                    for line in stdout_lines.push(&data) {
                                        if let Some(process_result) = parse_process_result(&line).map_err(|error| AttemptFailure {
                                            emitted_error: false,
                                            error,
                                            phase: Some(RunPhase::PersistOutputs),
                                            retriable: false,
                                            status: RunStatus::Failed,
                                        })? {
                                            success = Some(AttemptSuccess {
                                                output_path: process_result.output_path,
                                                validation_issues: process_result.validation_issues,
                                            });
                                            continue;
                                        }
                                        self.handle_runtime_event(
                                            &mut run,
                                            active,
                                            RunEventPayload::Log {
                                                level: "info".to_string(),
                                                message: line,
                                                phase: RunPhase::ExecuteRun,
                                            },
                                        )
                                        .await
                                        .map_err(store_failure)?;
                                    }
                                }
                                ChannelStream::Stderr => {
                                    for line in stderr_lines.push(&data) {
                                        self.handle_runtime_event(
                                            &mut run,
                                            active,
                                            RunEventPayload::Log {
                                                level: "error".to_string(),
                                                message: line,
                                                phase: RunPhase::ExecuteRun,
                                            },
                                        )
                                        .await
                                        .map_err(store_failure)?;
                                    }
                                }
                                ChannelStream::Pty => {}
                            }
                        }
                        Ok(ScopeSessionEvent::Exit { channel_id: event_channel_id, code }) if event_channel_id == channel_id => {
                            if let (Some(0), Some(success)) = (code, success) {
                                self.handle_runtime_event(
                                    &mut run,
                                    active,
                                    RunEventPayload::Status {
                                        phase: RunPhase::PersistOutputs,
                                        state: "started".to_string(),
                                        session_guid: None,
                                        operation_id: None,
                                        timings: None,
                                    },
                                )
                                .await
                                .map_err(store_failure)?;
                                self.handle_runtime_event(
                                    &mut run,
                                    active,
                                    RunEventPayload::Result {
                                        output_path: success.output_path.clone(),
                                        validation_issues: success.validation_issues.clone(),
                                    },
                                )
                                .await
                                .map_err(store_failure)?;
                                self.handle_runtime_event(
                                    &mut run,
                                    active,
                                    RunEventPayload::Status {
                                        phase: RunPhase::PersistOutputs,
                                        state: "completed".to_string(),
                                        session_guid: None,
                                        operation_id: None,
                                        timings: None,
                                    },
                                )
                                .await
                                .map_err(store_failure)?;
                                return Ok(success);
                            }

                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(
                                    StatusCode::BAD_GATEWAY,
                                    run_exit_message(code, &stdout_lines, &stderr_lines),
                                ),
                                phase: Some(RunPhase::ExecuteRun),
                                retriable: false,
                                status: RunStatus::Failed,
                            });
                        }
                        Ok(ScopeSessionEvent::Error { channel_id: Some(event_channel_id), message }) if event_channel_id == channel_id => {
                            self.handle_runtime_event(
                                &mut run,
                                active,
                                RunEventPayload::Error {
                                    phase: Some(RunPhase::ExecuteRun),
                                    message: message.clone(),
                                    retriable: false,
                                },
                            )
                            .await
                            .map_err(store_failure)?;
                            return Err(AttemptFailure {
                                emitted_error: true,
                                error: AppError::status(StatusCode::BAD_GATEWAY, message),
                                phase: Some(RunPhase::ExecuteRun),
                                retriable: false,
                                status: RunStatus::Failed,
                            });
                        }
                        Ok(ScopeSessionEvent::Error { channel_id: None, message }) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(StatusCode::BAD_GATEWAY, message),
                                phase: run.phase.or(Some(RunPhase::ExecuteRun)),
                                retriable: true,
                                status: RunStatus::Failed,
                            });
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(
                                    StatusCode::BAD_GATEWAY,
                                    "Scope session connector event stream overflowed while the run was active.",
                                ),
                                phase: run.phase.or(Some(RunPhase::ExecuteRun)),
                                retriable: true,
                                status: RunStatus::Failed,
                            });
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(
                                    StatusCode::BAD_GATEWAY,
                                    "Scope session connector disconnected while the run was active.",
                                ),
                                phase: run.phase.or(Some(RunPhase::ExecuteRun)),
                                retriable: true,
                                status: RunStatus::Failed,
                            });
                        }
                    }
                }
            }
        }
    }
}

fn resolve_artifact_access(
    app_url: &Url,
    access: ArtifactAccessGrant,
) -> Result<ArtifactAccessInstruction, AppError> {
    let url = match Url::parse(&access.url) {
        Ok(url) => url.to_string(),
        Err(_) => app_url
            .join(&access.url)
            .map_err(|error| {
                AppError::internal_with_source("Failed to resolve an artifact access URL.", error)
            })?
            .to_string(),
    };
    Ok(ArtifactAccessInstruction {
        expires_at: access.expires_at,
        headers: access.headers,
        method: access.method.to_string(),
        url,
    })
}

fn run_env(
    python_bin: String,
    input_access: ArtifactAccessInstruction,
    output_access: ArtifactAccessInstruction,
    local_input_path: String,
    local_output_dir: String,
    output_path: String,
) -> Result<BTreeMap<String, String>, AppError> {
    Ok(BTreeMap::from([
        (
            "ADE_INPUT_ACCESS_JSON".to_string(),
            serde_json::to_string(&input_access).map_err(|error| {
                AppError::internal_with_source("Failed to encode input artifact access.", error)
            })?,
        ),
        (
            "ADE_OUTPUT_ACCESS_JSON".to_string(),
            serde_json::to_string(&output_access).map_err(|error| {
                AppError::internal_with_source("Failed to encode output artifact access.", error)
            })?,
        ),
        ("ADE_LOCAL_INPUT_PATH".to_string(), local_input_path),
        ("ADE_LOCAL_OUTPUT_DIR".to_string(), local_output_dir),
        ("ADE_OUTPUT_PATH".to_string(), output_path),
        ("ADE_PYTHON_BIN".to_string(), python_bin),
    ]))
}

fn parse_process_result(line: &str) -> Result<Option<ProcessResult>, AppError> {
    let Some(payload) = line.strip_prefix(PROCESS_RESULT_PREFIX) else {
        return Ok(None);
    };
    serde_json::from_str(payload).map(Some).map_err(|error| {
        AppError::internal_with_source("Failed to parse the run result payload.", error)
    })
}

fn run_exit_message(
    code: Option<i32>,
    stdout_lines: &LineBuffer,
    stderr_lines: &LineBuffer,
) -> String {
    let stderr = stderr_lines.finish();
    if !stderr.is_empty() {
        return stderr.join("\n");
    }
    let stdout = stdout_lines.finish();
    if !stdout.is_empty() {
        return stdout.join("\n");
    }
    format!(
        "ADE run channel exited with code {}.",
        code.unwrap_or_default()
    )
}

fn uploaded_basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("input.bin")
        .to_string()
}

#[derive(Default)]
struct LineBuffer {
    buffer: Vec<u8>,
}

impl LineBuffer {
    fn push(&mut self, data: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(data);
        let mut lines = Vec::new();
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let drained = self.buffer.drain(..=index).collect::<Vec<_>>();
            let line = String::from_utf8_lossy(&drained)
                .trim_end_matches(['\r', '\n'])
                .to_string();
            if !line.is_empty() {
                lines.push(line);
            }
        }
        lines
    }

    fn finish(&self) -> Vec<String> {
        let tail = String::from_utf8_lossy(&self.buffer)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if tail.is_empty() {
            Vec::new()
        } else {
            vec![tail]
        }
    }
}

fn store_failure(error: AppError) -> AttemptFailure {
    AttemptFailure {
        emitted_error: false,
        error,
        phase: None,
        retriable: false,
        status: RunStatus::Failed,
    }
}

pub(super) fn attempt_failure(error: AppError, phase: Option<RunPhase>) -> AttemptFailure {
    AttemptFailure {
        emitted_error: false,
        error,
        phase,
        retriable: false,
        status: RunStatus::Failed,
    }
}
