use tokio::sync::broadcast;

use crate::error::AppError;

use super::{
    models::{
        PublicRunCompletedEvent, PublicRunCreatedEvent, PublicRunErrorEvent, PublicRunLogEvent,
        PublicRunResultEvent, PublicRunStatusEvent,
    },
    store::RunEvent,
};

pub(crate) struct RunEventFeed {
    pub(crate) replay: Vec<RunEvent>,
    pub(crate) subscription: Option<broadcast::Receiver<RunEvent>>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct ArchivedRunEventLine {
    data: serde_json::Value,
    event: String,
    seq: i64,
}

fn public_event_data(
    run_id: &str,
    event: &RunEvent,
) -> Result<(&'static str, serde_json::Value), AppError> {
    let (event_name, data) = match event {
        RunEvent::Created { status, .. } => (
            "run.created",
            serde_json::to_value(PublicRunCreatedEvent {
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
            serde_json::to_value(PublicRunStatusEvent {
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
            serde_json::to_value(PublicRunLogEvent {
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
            serde_json::to_value(PublicRunErrorEvent {
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
            serde_json::to_value(PublicRunResultEvent {
                output_path: output_path.clone(),
                run_id: run_id.to_string(),
                validation_issues: validation_issues.clone(),
            }),
        ),
        RunEvent::Complete {
            final_status,
            log_path,
            output_path,
            ..
        } => (
            "run.completed",
            serde_json::to_value(PublicRunCompletedEvent {
                final_status: *final_status,
                log_path: log_path.clone(),
                output_path: output_path.clone(),
                run_id: run_id.to_string(),
            }),
        ),
    };

    Ok((
        event_name,
        data.map_err(|error| {
            AppError::internal_with_source("Failed to encode a run event.", error)
        })?,
    ))
}

pub(crate) fn map_public_event(
    run_id: &str,
    event: &RunEvent,
) -> Result<(&'static str, String, String), AppError> {
    let (event_name, data) = public_event_data(run_id, event)?;
    Ok((
        event_name,
        event.seq().to_string(),
        serde_json::to_string(&data).map_err(|error| {
            AppError::internal_with_source("Failed to encode a run event.", error)
        })?,
    ))
}

pub(crate) fn archive_event_line(run_id: &str, event: &RunEvent) -> Result<String, AppError> {
    let (event_name, data) = public_event_data(run_id, event)?;
    serde_json::to_string(&ArchivedRunEventLine {
        data,
        event: event_name.to_string(),
        seq: event.seq(),
    })
    .map_err(|error| AppError::internal_with_source("Failed to encode a run event line.", error))
}

pub(crate) fn parse_archived_events(body: &str) -> Result<Vec<RunEvent>, AppError> {
    let mut replay = Vec::new();
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let line = serde_json::from_str::<ArchivedRunEventLine>(line).map_err(|error| {
            AppError::internal_with_source("Failed to decode an archived run event.", error)
        })?;
        replay.push(archived_event(line)?);
    }
    Ok(replay)
}

fn archived_event(line: ArchivedRunEventLine) -> Result<RunEvent, AppError> {
    match line.event.as_str() {
        "run.created" => {
            let event =
                serde_json::from_value::<PublicRunCreatedEvent>(line.data).map_err(|error| {
                    AppError::internal_with_source(
                        "Failed to decode an archived run.created event.",
                        error,
                    )
                })?;
            Ok(RunEvent::Created {
                seq: line.seq,
                status: event.status,
            })
        }
        "run.status" => {
            let event =
                serde_json::from_value::<PublicRunStatusEvent>(line.data).map_err(|error| {
                    AppError::internal_with_source(
                        "Failed to decode an archived run.status event.",
                        error,
                    )
                })?;
            Ok(RunEvent::Status {
                seq: line.seq,
                phase: event.phase,
                state: event.state,
                session_guid: event.session_guid,
                operation_id: event.operation_id,
                timings: event.timings,
            })
        }
        "run.log" => {
            let event =
                serde_json::from_value::<PublicRunLogEvent>(line.data).map_err(|error| {
                    AppError::internal_with_source(
                        "Failed to decode an archived run.log event.",
                        error,
                    )
                })?;
            Ok(RunEvent::Log {
                seq: line.seq,
                level: event.level,
                message: event.message,
                phase: event.phase,
            })
        }
        "run.error" => {
            let event =
                serde_json::from_value::<PublicRunErrorEvent>(line.data).map_err(|error| {
                    AppError::internal_with_source(
                        "Failed to decode an archived run.error event.",
                        error,
                    )
                })?;
            Ok(RunEvent::Error {
                seq: line.seq,
                phase: event.phase,
                message: event.message,
                retriable: event.retriable,
            })
        }
        "run.result" => {
            let event =
                serde_json::from_value::<PublicRunResultEvent>(line.data).map_err(|error| {
                    AppError::internal_with_source(
                        "Failed to decode an archived run.result event.",
                        error,
                    )
                })?;
            Ok(RunEvent::Result {
                seq: line.seq,
                output_path: event.output_path,
                validation_issues: event.validation_issues,
            })
        }
        "run.completed" => {
            let event =
                serde_json::from_value::<PublicRunCompletedEvent>(line.data).map_err(|error| {
                    AppError::internal_with_source(
                        "Failed to decode an archived run.completed event.",
                        error,
                    )
                })?;
            Ok(RunEvent::Complete {
                seq: line.seq,
                final_status: event.final_status,
                log_path: event.log_path,
                output_path: event.output_path,
            })
        }
        _ => Err(AppError::internal(format!(
            "Unsupported archived run event '{}'.",
            line.event
        ))),
    }
}
