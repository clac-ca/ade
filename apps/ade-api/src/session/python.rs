use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

use super::{ExecuteCommandResponse, client::PythonExecution};

pub(super) const COMMAND_SENTINEL_PREFIX: &str = "__ADE_COMMAND_META__=";
#[cfg(test)]
pub(super) const RUN_SENTINEL_PREFIX: &str = "__ADE_RUN_RESULT__=";

const COMMAND_TEMPLATE: &str = include_str!("command.py.tmpl");
#[cfg(test)]
const RUN_TEMPLATE: &str = include_str!("run.py.tmpl");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandMetadata {
    exit_code: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandTemplateConfig<'a> {
    command: &'a str,
    sentinel_prefix: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg(test)]
pub(super) struct RunPythonConfig {
    pub(super) config_package_name: &'static str,
    pub(super) config_version: String,
    pub(super) config_wheel_path: String,
    pub(super) engine_package_name: &'static str,
    pub(super) engine_version: String,
    pub(super) engine_wheel_path: String,
    pub(super) input_path: String,
    pub(super) install_lock_path: String,
    pub(super) runs_root: String,
    pub(super) sentinel_prefix: &'static str,
}

pub(super) fn command_execution_code(shell_command: &str) -> Result<String, AppError> {
    render_python_template(
        COMMAND_TEMPLATE,
        &CommandTemplateConfig {
            command: shell_command,
            sentinel_prefix: COMMAND_SENTINEL_PREFIX,
        },
    )
}

pub(super) fn extract_command_response(
    execution: PythonExecution,
) -> Result<ExecuteCommandResponse, AppError> {
    let Some((stdout, exit_code)) = strip_command_metadata(&execution.stdout)? else {
        let message = if !execution.stderr.trim().is_empty() {
            execution.stderr.trim().to_string()
        } else if !execution.stdout.trim().is_empty() {
            execution.stdout.trim().to_string()
        } else {
            format!(
                "Session pool command execution failed with status {}.",
                execution.status
            )
        };
        return Err(AppError::status(
            StatusCode::BAD_GATEWAY,
            format!("ADE command execution failed: {message}"),
        ));
    };

    Ok(ExecuteCommandResponse {
        duration_ms: execution.duration_ms,
        exit_code,
        stderr: execution.stderr,
        stdout,
    })
}

pub(super) fn strip_command_metadata(stdout: &str) -> Result<Option<(String, i64)>, AppError> {
    let Some(marker_index) = stdout.rfind(COMMAND_SENTINEL_PREFIX) else {
        return Ok(None);
    };

    let metadata = serde_json::from_str::<CommandMetadata>(
        stdout[marker_index + COMMAND_SENTINEL_PREFIX.len()..].trim(),
    )
    .map_err(|error| {
        AppError::internal_with_source("Failed to decode the command execution metadata.", error)
    })?;

    Ok(Some((
        stdout[..marker_index].trim_end_matches('\n').to_string(),
        metadata.exit_code,
    )))
}

#[cfg(test)]
pub(super) fn build_run_code(config: &RunPythonConfig) -> Result<String, AppError> {
    render_python_template(RUN_TEMPLATE, config)
}

fn render_python_template(template: &str, config: &impl Serialize) -> Result<String, AppError> {
    if !template.contains("{{CONFIG_JSON}}") {
        return Err(AppError::internal(
            "Python execution template is missing the CONFIG_JSON placeholder.",
        ));
    }

    let config_json = serde_json::to_string(config).map_err(|error| {
        AppError::internal_with_source("Failed to encode a Python template value.", error)
    })?;

    Ok(template.replace("{{CONFIG_JSON}}", &json_string(&config_json)?))
}

fn json_string(value: &str) -> Result<String, AppError> {
    serde_json::to_string(value).map_err(|error| {
        AppError::internal_with_source("Failed to encode a Python template value.", error)
    })
}

#[cfg(test)]
pub(super) fn ensure_successful_execution(execution: &PythonExecution) -> Result<(), AppError> {
    if matches!(execution.status.as_str(), "Success" | "Succeeded" | "0") {
        return Ok(());
    }

    let message = if !execution.stderr.trim().is_empty() {
        execution.stderr.trim().to_string()
    } else if !execution.stdout.trim().is_empty() {
        execution.stdout.trim().to_string()
    } else {
        format!(
            "Session pool execution failed with status {}.",
            execution.status
        )
    };

    Err(AppError::status(
        StatusCode::BAD_GATEWAY,
        format!("ADE run execution failed: {message}"),
    ))
}

#[cfg(test)]
pub(super) fn extract_run_response(
    execution: &PythonExecution,
) -> Result<super::RunResponse, AppError> {
    let marker_index = execution.stdout.rfind(RUN_SENTINEL_PREFIX).ok_or_else(|| {
        AppError::internal("ADE run execution did not emit the structured result metadata.")
    })?;
    serde_json::from_str::<super::RunResponse>(
        execution.stdout[marker_index + RUN_SENTINEL_PREFIX.len()..].trim(),
    )
    .map_err(|error| {
        AppError::internal_with_source("Failed to decode the ADE run metadata.", error)
    })
}
