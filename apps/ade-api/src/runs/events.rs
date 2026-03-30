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

pub(crate) fn run_events_path(workspace_id: &str, config_version_id: &str, run_id: &str) -> String {
    format!("/api/workspaces/{workspace_id}/configs/{config_version_id}/runs/{run_id}/events")
}

#[cfg(test)]
mod tests {
    use super::run_events_path;

    #[test]
    fn run_events_urls_are_stable() {
        assert_eq!(
            run_events_path("workspace-a", "config-v1", "run-1"),
            "/api/workspaces/workspace-a/configs/config-v1/runs/run-1/events"
        );
    }
}
