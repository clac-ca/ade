use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::runs::{RunPhase, RunValidationIssue};

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct WorkerId(pub String);

impl WorkerId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerKind {
    Run,
    Shell,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArtifactAccess {
    pub headers: BTreeMap<String, String>,
    pub method: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionAgentCommand {
    Prepare {
        config_package_name: String,
        config_version: String,
        config_wheel_path: String,
        engine_package_name: String,
        engine_version: String,
        engine_wheel_path: String,
        python_executable_path: String,
        python_home_path: String,
        python_toolchain_path: String,
        python_toolchain_version: String,
    },
    StartShell {
        cols: u16,
        cwd: String,
        rows: u16,
        worker_id: WorkerId,
    },
    WriteInput {
        data: String,
        worker_id: WorkerId,
    },
    ResizePty {
        cols: u16,
        rows: u16,
        worker_id: WorkerId,
    },
    CloseWorker {
        worker_id: WorkerId,
    },
    StartRun {
        input_download: SessionArtifactAccess,
        local_input_path: String,
        local_output_dir: String,
        output_path: String,
        output_upload: SessionArtifactAccess,
        timeout_in_seconds: u64,
        worker_id: WorkerId,
    },
    CancelWorker {
        worker_id: WorkerId,
    },
    Shutdown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionAgentEvent {
    Ready,
    Prepared {
        config_version: String,
        engine_version: String,
        python_toolchain_version: String,
    },
    Log {
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<RunPhase>,
        level: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        worker_id: Option<WorkerId>,
    },
    Stdout {
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<RunPhase>,
        worker_id: WorkerId,
    },
    Stderr {
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<RunPhase>,
        worker_id: WorkerId,
    },
    PtyOutput {
        data: String,
        worker_id: WorkerId,
    },
    WorkerExit {
        code: Option<i32>,
        kind: WorkerKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        validation_issues: Option<Vec<RunValidationIssue>>,
        worker_id: WorkerId,
    },
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<RunPhase>,
        retriable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        worker_id: Option<WorkerId>,
    },
}
