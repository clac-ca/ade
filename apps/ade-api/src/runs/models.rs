use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::artifacts::ArtifactAccessGrant;

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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateUploadBatchFile {
    pub content_type: Option<String>,
    pub filename: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactAccessInstruction {
    pub expires_at: String,
    pub headers: BTreeMap<String, String>,
    pub method: String,
    pub url: String,
}

impl From<ArtifactAccessGrant> for ArtifactAccessInstruction {
    fn from(value: ArtifactAccessGrant) -> Self {
        Self {
            expires_at: value.expires_at,
            headers: value.headers,
            method: value.method.to_string(),
            url: value.url,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateUploadResponse {
    pub file_path: String,
    pub upload: ArtifactAccessInstruction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateUploadBatchRequest {
    pub files: Vec<CreateUploadBatchFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateUploadBatchItem {
    pub file_id: String,
    pub file_path: String,
    pub upload: ArtifactAccessInstruction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateUploadBatchResponse {
    pub batch_id: String,
    pub items: Vec<CreateUploadBatchItem>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RunArtifactKind {
    Log,
    Output,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateDownloadRequest {
    pub artifact: RunArtifactKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateDownloadResponse {
    pub download: ArtifactAccessInstruction,
    pub file_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AsyncRunResponse {
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
    pub log_path: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    pub run_id: String,
}
