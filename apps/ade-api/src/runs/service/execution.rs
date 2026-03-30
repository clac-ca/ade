use super::*;

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

impl RunService {
    async fn consume_run_bridge(
        &self,
        run_id: &str,
        run: &mut StoredRun,
        active: &ActiveRunHandle,
        attempt_session_guid: &mut Option<String>,
        mut bridge_socket: WebSocket,
        mut execution_task: JoinHandle<Result<SessionOperationResult<PythonExecution>, AppError>>,
    ) -> Result<AttemptSuccess, AttemptFailure> {
        loop {
            match bridge_socket.recv().await {
                Some(Ok(Message::Text(text))) => {
                    let message = match parse_bridge_message(text.as_str()) {
                        Ok(message) => message,
                        Err(error) => {
                            abandon_execution_task(&mut execution_task).await;
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error,
                                phase: Some(RunPhase::InstallPackages),
                                retriable: false,
                            });
                        }
                    };
                    match message {
                        RunBridgeClientMessage::Ready => break,
                        _ => {
                            let _ = send_json(&mut bridge_socket, &RunBridgeServerMessage::Cancel)
                                .await;
                            abandon_execution_task(&mut execution_task).await;
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
                    abandon_execution_task(&mut execution_task).await;
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
                    abandon_execution_task(&mut execution_task).await;
                    return Err(AttemptFailure {
                        emitted_error: false,
                        error: AppError::request("Binary run bridge messages are not supported."),
                        phase: Some(RunPhase::InstallPackages),
                        retriable: false,
                    });
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                Some(Err(error)) => {
                    abandon_execution_task(&mut execution_task).await;
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
        let mut execution: Option<SessionOperationResult<PythonExecution>> = None;
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
                            let message = match parse_bridge_message(text.as_str()) {
                                Ok(message) => message,
                                Err(error) => {
                                    abandon_execution_task(&mut execution_task).await;
                                    return Err(AttemptFailure {
                                        emitted_error: false,
                                        error,
                                        phase: run.phase.or(Some(RunPhase::InstallPackages)),
                                        retriable: false,
                                    });
                                }
                            };
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
                            abandon_execution_task(&mut execution_task).await;
                            return Err(AttemptFailure {
                                emitted_error: false,
                                error: AppError::request("Binary run bridge messages are not supported."),
                                phase: run.phase.or(Some(RunPhase::InstallPackages)),
                                retriable: false,
                            });
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                        Some(Err(error)) => {
                            abandon_execution_task(&mut execution_task).await;
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
                    if active.is_cancelled() {
                        return self.finish_cancelled(&scope, &run_id, Some(&active)).await;
                    }
                    return self
                        .finish_success(&scope, &run_id, attempt, success, Some(&active))
                        .await;
                }
                Err(failure) => failure,
            };

            if active.is_cancelled() {
                return self.finish_cancelled(&scope, &run_id, Some(&active)).await;
            }

            if !(failure.retriable && attempt < RUN_MAX_ATTEMPTS) {
                return self
                    .finish_failure(
                        &scope,
                        &run_id,
                        attempt,
                        &input_path,
                        failure,
                        Some(&active),
                    )
                    .await;
            }
        }

        Err(AppError::status(StatusCode::BAD_GATEWAY, "ADE run failed."))
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
        run.output_path = None;
        run.validation_issues.clear();
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
        run.output_path = None;
        run.validation_issues.clear();
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
        let bridge_url = self.build_bridge_url(&pending.bridge_id);
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
        let mut execution_task = tokio::spawn(async move {
            session_service
                .execute_inline_python_detailed(
                    &scope_for_execution,
                    bootstrap,
                    Some(execution_timeout),
                )
                .await
        });

        let bridge_socket = match self.wait_for_run_bridge(pending, active).await {
            Ok(bridge_socket) => bridge_socket,
            Err(error) => {
                abandon_execution_task(&mut execution_task).await;
                return Err(AttemptFailure {
                    emitted_error: false,
                    error,
                    phase: Some(RunPhase::InstallPackages),
                    retriable: true,
                });
            }
        };
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
            .upload_session_file(
                scope,
                &runtime.engine_filename,
                Some("application/octet-stream".to_string()),
                runtime.engine_wheel_bytes.clone(),
            )
            .await?;
        note_session_guid(attempt_session_guid, &engine_upload.metadata)?;

        let config_upload = self
            .session_service
            .upload_session_file(
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

    fn build_bridge_url(&self, bridge_id: &str) -> String {
        let expires_at_ms = unix_time_ms() + BRIDGE_TOKEN_TTL_MS;
        let token = create_bridge_token(&self.session_secret, bridge_id, expires_at_ms);
        let mut bridge_url = self.app_url.clone();
        let scheme = if bridge_url.scheme() == "http" {
            "ws"
        } else {
            "wss"
        };
        bridge_url
            .set_scheme(scheme)
            .expect("ADE_APP_URL scheme was validated at startup");
        bridge_url.set_path(&format!("/api/internal/run-bridges/{bridge_id}"));
        bridge_url.set_query(None);
        bridge_url.query_pairs_mut().append_pair("token", &token);
        bridge_url.to_string()
    }
}

fn cancelled_failure() -> AttemptFailure {
    AttemptFailure {
        emitted_error: true,
        error: AppError::request("Run cancelled."),
        phase: None,
        retriable: false,
    }
}

async fn abandon_execution_task<T>(execution_task: &mut JoinHandle<T>) {
    if !execution_task.is_finished() {
        execution_task.abort();
    }
    let _ = execution_task.await;
}

fn execution_failure_message(execution: &PythonExecution) -> String {
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

fn store_failure(error: AppError) -> AttemptFailure {
    AttemptFailure {
        emitted_error: false,
        error,
        phase: None,
        retriable: false,
    }
}
