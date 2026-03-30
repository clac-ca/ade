use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{RunPhase, RunStatus, RunTimings};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRunRequest {
    pub input_path: String,
    pub timeout_in_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateUploadRequest {
    pub content_type: Option<String>,
    pub filename: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UploadInstruction {
    pub expires_at: String,
    pub headers: BTreeMap<String, String>,
    pub method: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateUploadResponse {
    pub file_path: String,
    pub upload: UploadInstruction,
    pub upload_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AsyncRunResponse {
    pub events_url: String,
    pub input_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    pub run_id: String,
    pub status: RunStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDetailResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub input_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<RunPhase>,
    pub run_id: String,
    pub status: RunStatus,
    pub validation_issues: Vec<RunValidationIssue>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunValidationIssue {
    pub row_index: usize,
    pub field: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunCreatedEvent {
    pub run_id: String,
    pub status: RunStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunStatusEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub phase: RunPhase,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_guid: Option<String>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timings: Option<RunTimings>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunLogEvent {
    pub level: String,
    pub message: String,
    pub phase: RunPhase,
    pub run_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunErrorEvent {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<RunPhase>,
    pub retriable: bool,
    pub run_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunResultEvent {
    pub output_path: String,
    pub run_id: String,
    pub validation_issues: Vec<RunValidationIssue>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRunCompletedEvent {
    pub final_status: RunStatus,
    pub run_id: String,
}
