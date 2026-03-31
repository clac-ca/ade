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

#[derive(serde::Serialize)]
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
