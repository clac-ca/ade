use super::*;

use crate::{
    artifacts::ArtifactAccessGrant,
    session_agent::{
        SessionAgentCommand, SessionAgentEvent, SessionArtifactAccess, WorkerId, WorkerKind,
    },
};

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
                unreachable!("runtime bridge does not emit created or completed events")
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
        let runtime = self
            .session_agent_service
            .runtime_artifacts(attempt.scope)
            .map_err(|error| AttemptFailure {
                emitted_error: false,
                error,
                phase: Some(RunPhase::InstallPackages),
                retriable: false,
                status: RunStatus::Failed,
            })?;
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

        let handle = self
            .session_agent_service
            .connect_scope_session(attempt.scope, &runtime)
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
                state: "started".to_string(),
                session_guid: None,
                operation_id: None,
                timings: None,
            },
        )
        .await
        .map_err(store_failure)?;

        self.session_agent_service
            .ensure_prepared(attempt.scope, &runtime, &handle)
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
        let worker_id = WorkerId::new(format!(
            "run-{}-attempt-{}",
            attempt.run_id, attempt.attempt
        ));
        let mut events = handle.subscribe();
        handle
            .send(SessionAgentCommand::StartRun {
                input_download: resolve_artifact_access(&self.app_url, input_download).map_err(
                    |error| AttemptFailure {
                        emitted_error: false,
                        error,
                        phase: Some(RunPhase::ExecuteRun),
                        retriable: false,
                        status: RunStatus::Failed,
                    },
                )?,
                local_input_path: format!(
                    "/mnt/data/work/input/{}/attempt-{}/{}",
                    attempt.run_id,
                    attempt.attempt,
                    uploaded_basename(attempt.input_path)
                ),
                local_output_dir: format!(
                    "/mnt/data/runs/{}/attempt-{}/output",
                    attempt.run_id, attempt.attempt
                ),
                output_path: attempt.output_path.to_string(),
                output_upload: resolve_artifact_access(&self.app_url, output_upload).map_err(
                    |error| AttemptFailure {
                        emitted_error: false,
                        error,
                        phase: Some(RunPhase::PersistOutputs),
                        retriable: false,
                        status: RunStatus::Failed,
                    },
                )?,
                timeout_in_seconds: run_timeout_in_seconds(attempt.timeout_in_seconds),
                worker_id: worker_id.clone(),
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

        loop {
            tokio::select! {
                _ = cancel_rx.changed() => {
                    let _ = handle.send(SessionAgentCommand::CancelWorker { worker_id: worker_id.clone() }).await;
                    return Err(AttemptFailure {
                        emitted_error: true,
                        error: AppError::request("Run cancelled."),
                        phase: None,
                        retriable: false,
                        status: RunStatus::Cancelled,
                    });
                }
                event = events.recv() => {
                    match event {
                        Ok(SessionAgentEvent::Stdout { worker_id: event_worker_id, data, phase }) if event_worker_id == worker_id => {
                            for line in streamed_lines(&data) {
                                self.handle_runtime_event(
                                    &mut run,
                                    active,
                                    RunEventPayload::Log {
                                        level: "info".to_string(),
                                        message: line,
                                        phase: phase.unwrap_or(RunPhase::ExecuteRun),
                                    },
                                )
                                .await
                                .map_err(store_failure)?;
                            }
                        }
                        Ok(SessionAgentEvent::Stderr { worker_id: event_worker_id, data, phase }) if event_worker_id == worker_id => {
                            for line in streamed_lines(&data) {
                                self.handle_runtime_event(
                                    &mut run,
                                    active,
                                    RunEventPayload::Log {
                                        level: "error".to_string(),
                                        message: line,
                                        phase: phase.unwrap_or(RunPhase::ExecuteRun),
                                    },
                                )
                                .await
                                .map_err(store_failure)?;
                            }
                        }
                        Ok(SessionAgentEvent::Log { worker_id: Some(event_worker_id), level, message, phase }) if event_worker_id == worker_id => {
                            self.handle_runtime_event(
                                &mut run,
                                active,
                                RunEventPayload::Log {
                                    level,
                                    message,
                                    phase: phase.unwrap_or(RunPhase::ExecuteRun),
                                },
                            )
                            .await
                            .map_err(store_failure)?;
                        }
                        Ok(SessionAgentEvent::Error { worker_id: Some(event_worker_id), phase, message, retriable }) if event_worker_id == worker_id => {
                            self.handle_runtime_event(
                                &mut run,
                                active,
                                RunEventPayload::Error {
                                    phase,
                                    message: message.clone(),
                                    retriable,
                                },
                            )
                            .await
                            .map_err(store_failure)?;
                            return Err(AttemptFailure {
                                emitted_error: true,
                                error: AppError::status(StatusCode::BAD_GATEWAY, message),
                                phase,
                                retriable,
                                status: RunStatus::Failed,
                            });
                        }
                        Ok(SessionAgentEvent::WorkerExit { worker_id: event_worker_id, kind: WorkerKind::Run, code, output_path, validation_issues, .. }) if event_worker_id == worker_id => {
                            if let (Some(0), Some(output_path)) = (code, output_path) {
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
                                let validation_issues = validation_issues.unwrap_or_default();
                                self.handle_runtime_event(
                                    &mut run,
                                    active,
                                    RunEventPayload::Result {
                                        output_path: output_path.clone(),
                                        validation_issues: validation_issues.clone(),
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
                                return Ok(AttemptSuccess { output_path, validation_issues });
                            }

                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(
                                    StatusCode::BAD_GATEWAY,
                                    format!("ADE run worker exited with code {}.", code.unwrap_or_default()),
                                ),
                                phase: Some(RunPhase::ExecuteRun),
                                retriable: false,
                                status: RunStatus::Failed,
                            });
                        }
                        Ok(SessionAgentEvent::Error { worker_id: None, message, phase, retriable }) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(StatusCode::BAD_GATEWAY, message),
                                phase,
                                retriable,
                                status: RunStatus::Failed,
                            });
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(
                                    StatusCode::BAD_GATEWAY,
                                    "Scope session agent event stream overflowed while the run was active.",
                                ),
                                phase: run.phase.or(Some(RunPhase::ExecuteRun)),
                                retriable: true,
                                status: RunStatus::Failed,
                            });
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::status(
                                    StatusCode::BAD_GATEWAY,
                                    "Scope session agent disconnected while the run was active.",
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
) -> Result<SessionArtifactAccess, AppError> {
    let url = match Url::parse(&access.url) {
        Ok(url) => url.to_string(),
        Err(_) => app_url
            .join(&access.url)
            .map_err(|error| {
                AppError::internal_with_source("Failed to resolve an artifact access URL.", error)
            })?
            .to_string(),
    };
    Ok(SessionArtifactAccess {
        headers: access.headers,
        method: access.method.to_string(),
        url,
    })
}

fn streamed_lines(data: &str) -> Vec<String> {
    data.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn uploaded_basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("input.bin")
        .to_string()
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
