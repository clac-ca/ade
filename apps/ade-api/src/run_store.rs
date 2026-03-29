use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    db::Database,
    error::AppError,
    session::{RunValidationIssue, Scope},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    Cancelled,
    Failed,
    Pending,
    Running,
    Succeeded,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
        }
    }

    fn from_str(value: &str) -> Result<Self, AppError> {
        match value {
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            _ => Err(AppError::internal(format!(
                "Unsupported run status '{value}'."
            ))),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed | Self::Succeeded)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum RunPhase {
    ExecuteRun,
    InstallPackages,
    PersistOutputs,
    UploadArtifacts,
}

impl RunPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::ExecuteRun => "executeRun",
            Self::InstallPackages => "installPackages",
            Self::PersistOutputs => "persistOutputs",
            Self::UploadArtifacts => "uploadArtifacts",
        }
    }

    fn from_str(value: &str) -> Result<Self, AppError> {
        match value {
            "executeRun" => Ok(Self::ExecuteRun),
            "installPackages" => Ok(Self::InstallPackages),
            "persistOutputs" => Ok(Self::PersistOutputs),
            "uploadArtifacts" => Ok(Self::UploadArtifacts),
            _ => Err(AppError::internal(format!(
                "Unsupported run phase '{value}'."
            ))),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunTimings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocation_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_execution_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overall_execution_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preparation_time_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum RunEvent {
    Created {
        seq: i64,
        status: RunStatus,
    },
    Complete {
        seq: i64,
        #[serde(rename = "finalStatus")]
        final_status: RunStatus,
    },
    Error {
        seq: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<RunPhase>,
        message: String,
        retriable: bool,
    },
    Log {
        seq: i64,
        level: String,
        message: String,
        phase: RunPhase,
    },
    Result {
        seq: i64,
        #[serde(rename = "outputPath")]
        output_path: String,
        #[serde(rename = "validationIssues")]
        validation_issues: Vec<RunValidationIssue>,
    },
    Status {
        seq: i64,
        phase: RunPhase,
        state: String,
        #[serde(rename = "sessionGuid")]
        #[serde(skip_serializing_if = "Option::is_none")]
        session_guid: Option<String>,
        #[serde(rename = "operationId")]
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timings: Option<RunTimings>,
    },
}

impl RunEvent {
    pub fn seq(&self) -> i64 {
        match self {
            Self::Created { seq, .. }
            | Self::Complete { seq, .. }
            | Self::Error { seq, .. }
            | Self::Log { seq, .. }
            | Self::Result { seq, .. }
            | Self::Status { seq, .. } => *seq,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum RunEventPayload {
    Created {
        status: RunStatus,
    },
    Complete {
        #[serde(rename = "finalStatus")]
        final_status: RunStatus,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
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
        #[serde(rename = "sessionGuid")]
        #[serde(skip_serializing_if = "Option::is_none")]
        session_guid: Option<String>,
        #[serde(rename = "operationId")]
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timings: Option<RunTimings>,
    },
}

impl RunEventPayload {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "created",
            Self::Complete { .. } => "complete",
            Self::Error { .. } => "error",
            Self::Log { .. } => "log",
            Self::Result { .. } => "result",
            Self::Status { .. } => "status",
        }
    }

    fn with_seq(self, seq: i64) -> RunEvent {
        match self {
            Self::Created { status } => RunEvent::Created { seq, status },
            Self::Complete { final_status } => RunEvent::Complete { seq, final_status },
            Self::Error {
                phase,
                message,
                retriable,
            } => RunEvent::Error {
                seq,
                phase,
                message,
                retriable,
            },
            Self::Log {
                level,
                message,
                phase,
            } => RunEvent::Log {
                seq,
                level,
                message,
                phase,
            },
            Self::Result {
                output_path,
                validation_issues,
            } => RunEvent::Result {
                seq,
                output_path,
                validation_issues,
            },
            Self::Status {
                phase,
                state,
                session_guid,
                operation_id,
                timings,
            } => RunEvent::Status {
                seq,
                phase,
                state,
                session_guid,
                operation_id,
                timings,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoredRun {
    pub attempt_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub input_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_session_guid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<RunPhase>,
    pub run_id: String,
    pub status: RunStatus,
    pub validation_issues: Vec<RunValidationIssue>,
    pub workspace_id: String,
    pub config_version_id: String,
}

impl StoredRun {
    pub fn scope(&self) -> Scope {
        Scope {
            workspace_id: self.workspace_id.clone(),
            config_version_id: self.config_version_id.clone(),
        }
    }
}

#[async_trait]
pub trait RunStore: Send + Sync {
    async fn append_event(
        &self,
        run_id: &str,
        event: RunEventPayload,
    ) -> Result<RunEvent, AppError>;
    async fn create_run(
        &self,
        scope: &Scope,
        run_id: &str,
        input_path: &str,
    ) -> Result<StoredRun, AppError>;
    async fn get_run(&self, scope: &Scope, run_id: &str) -> Result<Option<StoredRun>, AppError>;
    async fn last_event_seq(&self, run_id: &str) -> Result<Option<i64>, AppError>;
    async fn list_events_after(
        &self,
        run_id: &str,
        after_seq: Option<i64>,
    ) -> Result<Vec<RunEvent>, AppError>;
    async fn save_run(&self, run: &StoredRun) -> Result<(), AppError>;
}

pub(crate) type RunStoreHandle = Arc<dyn RunStore>;

pub struct InMemoryRunStore {
    events: Mutex<HashMap<String, Vec<RunEvent>>>,
    next_seq: AtomicI64,
    runs: Mutex<HashMap<String, StoredRun>>,
}

impl Default for InMemoryRunStore {
    fn default() -> Self {
        Self {
            events: Mutex::new(HashMap::new()),
            next_seq: AtomicI64::new(1),
            runs: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl RunStore for InMemoryRunStore {
    async fn append_event(
        &self,
        run_id: &str,
        event: RunEventPayload,
    ) -> Result<RunEvent, AppError> {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let event = event.with_seq(seq);
        let mut events = self.events.lock().unwrap();
        events
            .entry(run_id.to_string())
            .or_default()
            .push(event.clone());
        Ok(event)
    }

    async fn create_run(
        &self,
        scope: &Scope,
        run_id: &str,
        input_path: &str,
    ) -> Result<StoredRun, AppError> {
        let run = StoredRun {
            attempt_count: 0,
            error_message: None,
            input_path: input_path.to_string(),
            last_session_guid: None,
            output_path: None,
            phase: None,
            run_id: run_id.to_string(),
            status: RunStatus::Pending,
            validation_issues: Vec::new(),
            workspace_id: scope.workspace_id.clone(),
            config_version_id: scope.config_version_id.clone(),
        };
        self.runs
            .lock()
            .unwrap()
            .insert(run.run_id.clone(), run.clone());
        Ok(run)
    }

    async fn get_run(&self, scope: &Scope, run_id: &str) -> Result<Option<StoredRun>, AppError> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .get(run_id)
            .filter(|run| {
                run.workspace_id == scope.workspace_id
                    && run.config_version_id == scope.config_version_id
            })
            .cloned())
    }

    async fn last_event_seq(&self, run_id: &str) -> Result<Option<i64>, AppError> {
        Ok(self
            .events
            .lock()
            .unwrap()
            .get(run_id)
            .and_then(|events| events.last().map(RunEvent::seq)))
    }

    async fn list_events_after(
        &self,
        run_id: &str,
        after_seq: Option<i64>,
    ) -> Result<Vec<RunEvent>, AppError> {
        Ok(self
            .events
            .lock()
            .unwrap()
            .get(run_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|event| after_seq.is_none_or(|seq| event.seq() > seq))
            .collect())
    }

    async fn save_run(&self, run: &StoredRun) -> Result<(), AppError> {
        self.runs
            .lock()
            .unwrap()
            .insert(run.run_id.clone(), run.clone());
        Ok(())
    }
}

pub struct SqlRunStore {
    database: Arc<Database>,
}

impl SqlRunStore {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }
}

#[async_trait]
impl RunStore for SqlRunStore {
    async fn append_event(
        &self,
        run_id: &str,
        event: RunEventPayload,
    ) -> Result<RunEvent, AppError> {
        let event_type = event.event_type();
        let payload_json = serde_json::to_string(&event).map_err(|error| {
            AppError::internal_with_source("Failed to encode a run event.", error)
        })?;
        let row = self
            .database
            .query_optional(
                "INSERT INTO ade.run_events (run_id, event_type, payload_json) OUTPUT INSERTED.seq VALUES (@P1, @P2, @P3)",
                &[&run_id, &event_type, &payload_json],
            )
            .await?
            .ok_or_else(|| AppError::internal("Failed to insert a run event."))?;
        let seq: i64 = row
            .get(0)
            .ok_or_else(|| AppError::internal("Failed to read the run event sequence."))?;
        Ok(event.with_seq(seq))
    }

    async fn create_run(
        &self,
        scope: &Scope,
        run_id: &str,
        input_path: &str,
    ) -> Result<StoredRun, AppError> {
        let run = StoredRun {
            attempt_count: 0,
            error_message: None,
            input_path: input_path.to_string(),
            last_session_guid: None,
            output_path: None,
            phase: None,
            run_id: run_id.to_string(),
            status: RunStatus::Pending,
            validation_issues: Vec::new(),
            workspace_id: scope.workspace_id.clone(),
            config_version_id: scope.config_version_id.clone(),
        };
        self.database
            .execute(
                "INSERT INTO ade.runs (run_id, workspace_id, config_version_id, input_path, status, attempt_count, validation_issues_json, created_at, updated_at) VALUES (@P1, @P2, @P3, @P4, @P5, @P6, @P7, SYSUTCDATETIME(), SYSUTCDATETIME())",
                &[
                    &run.run_id,
                    &run.workspace_id,
                    &run.config_version_id,
                    &run.input_path,
                    &run.status.as_str(),
                    &run.attempt_count,
                    &"[]",
                ],
            )
            .await?;
        Ok(run)
    }

    async fn get_run(&self, scope: &Scope, run_id: &str) -> Result<Option<StoredRun>, AppError> {
        let row = self
            .database
            .query_optional(
                "SELECT run_id, workspace_id, config_version_id, input_path, status, phase, attempt_count, last_session_guid, output_path, validation_issues_json, error_message FROM ade.runs WHERE run_id = @P1 AND workspace_id = @P2 AND config_version_id = @P3",
                &[&run_id, &scope.workspace_id, &scope.config_version_id],
            )
            .await?;
        row.map(run_from_row).transpose()
    }

    async fn last_event_seq(&self, run_id: &str) -> Result<Option<i64>, AppError> {
        let row = self
            .database
            .query_optional(
                "SELECT MAX(seq) FROM ade.run_events WHERE run_id = @P1",
                &[&run_id],
            )
            .await?;
        Ok(row.and_then(|row| row.get(0)))
    }

    async fn list_events_after(
        &self,
        run_id: &str,
        after_seq: Option<i64>,
    ) -> Result<Vec<RunEvent>, AppError> {
        let min_seq = after_seq.unwrap_or(0);
        let rows = self
            .database
            .query_all(
                "SELECT seq, event_type, payload_json FROM ade.run_events WHERE run_id = @P1 AND seq > @P2 ORDER BY seq ASC",
                &[&run_id, &min_seq],
            )
            .await?;
        rows.into_iter().map(run_event_from_row).collect()
    }

    async fn save_run(&self, run: &StoredRun) -> Result<(), AppError> {
        let phase = run.phase.map(RunPhase::as_str);
        let validation_issues_json =
            serde_json::to_string(&run.validation_issues).map_err(|error| {
                AppError::internal_with_source(
                    "Failed to encode run validation issues.".to_string(),
                    error,
                )
            })?;
        self.database
            .execute(
                "UPDATE ade.runs SET status = @P2, phase = @P3, attempt_count = @P4, last_session_guid = @P5, output_path = @P6, validation_issues_json = @P7, error_message = @P8, updated_at = SYSUTCDATETIME() WHERE run_id = @P1",
                &[
                    &run.run_id,
                    &run.status.as_str(),
                    &phase,
                    &run.attempt_count,
                    &run.last_session_guid,
                    &run.output_path,
                    &validation_issues_json,
                    &run.error_message,
                ],
            )
            .await
    }
}

fn run_from_row(row: tiberius::Row) -> Result<StoredRun, AppError> {
    let validation_issues_json: &str = row
        .get(9)
        .ok_or_else(|| AppError::internal("Failed to read run validation issues."))?;
    let validation_issues = serde_json::from_str::<Vec<RunValidationIssue>>(validation_issues_json)
        .map_err(|error| {
            AppError::internal_with_source("Failed to decode run validation issues.", error)
        })?;

    Ok(StoredRun {
        run_id: row
            .get::<&str, _>(0)
            .ok_or_else(|| AppError::internal("Failed to read run id."))?
            .to_string(),
        workspace_id: row
            .get::<&str, _>(1)
            .ok_or_else(|| AppError::internal("Failed to read workspace id."))?
            .to_string(),
        config_version_id: row
            .get::<&str, _>(2)
            .ok_or_else(|| AppError::internal("Failed to read config version id."))?
            .to_string(),
        input_path: row
            .get::<&str, _>(3)
            .ok_or_else(|| AppError::internal("Failed to read input path."))?
            .to_string(),
        status: RunStatus::from_str(
            row.get::<&str, _>(4)
                .ok_or_else(|| AppError::internal("Failed to read run status."))?,
        )?,
        phase: row.get::<&str, _>(5).map(RunPhase::from_str).transpose()?,
        attempt_count: row
            .get(6)
            .ok_or_else(|| AppError::internal("Failed to read run attempt count."))?,
        last_session_guid: row.get::<&str, _>(7).map(ToOwned::to_owned),
        output_path: row.get::<&str, _>(8).map(ToOwned::to_owned),
        validation_issues,
        error_message: row.get::<&str, _>(10).map(ToOwned::to_owned),
    })
}

fn run_event_from_row(row: tiberius::Row) -> Result<RunEvent, AppError> {
    let seq: i64 = row
        .get(0)
        .ok_or_else(|| AppError::internal("Failed to read the run event sequence."))?;
    let payload_json: &str = row
        .get(2)
        .ok_or_else(|| AppError::internal("Failed to read the run event payload."))?;
    let event_type: &str = row
        .get(1)
        .ok_or_else(|| AppError::internal("Failed to read the run event type."))?;
    let payload = match event_type {
        "complete" | "created" | "error" | "log" | "result" | "status" => {
            serde_json::from_str::<RunEventPayload>(payload_json).map_err(|error| {
                AppError::internal_with_source("Failed to decode a run event payload.", error)
            })?
        }
        _ => {
            return Err(AppError::internal(format!(
                "Unsupported run event '{event_type}'."
            )));
        }
    };
    Ok(payload.with_seq(seq))
}

#[cfg(test)]
mod tests {
    use super::{RunEventPayload, RunPhase, RunStatus};

    #[test]
    fn statuses_round_trip() {
        assert_eq!(
            RunStatus::from_str(RunStatus::Pending.as_str()).unwrap(),
            RunStatus::Pending
        );
        assert_eq!(
            RunPhase::from_str(RunPhase::UploadArtifacts.as_str()).unwrap(),
            RunPhase::UploadArtifacts
        );
    }

    #[test]
    fn event_types_match_wire_names() {
        assert_eq!(
            RunEventPayload::Status {
                phase: RunPhase::UploadArtifacts,
                state: "started".to_string(),
                session_guid: None,
                operation_id: None,
                timings: None,
            }
            .event_type(),
            "status"
        );
        assert_eq!(
            RunEventPayload::Created {
                status: RunStatus::Pending
            }
            .event_type(),
            "created"
        );
    }
}
