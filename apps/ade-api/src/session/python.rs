use serde::Serialize;

use crate::error::AppError;

pub(super) const RUN_SENTINEL_PREFIX: &str = "__ADE_RUN_RESULT__=";

const RUN_TEMPLATE: &str = include_str!("run.py.tmpl");

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
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

pub(super) fn build_run_code(config: &RunPythonConfig) -> Result<String, AppError> {
    if !RUN_TEMPLATE.contains("{{CONFIG_JSON}}") {
        return Err(AppError::internal(
            "Python execution template is missing the CONFIG_JSON placeholder.",
        ));
    }

    let config_json = serde_json::to_string(config).map_err(|error| {
        AppError::internal_with_source("Failed to encode a Python template value.", error)
    })?;
    let encoded = serde_json::to_string(&config_json).map_err(|error| {
        AppError::internal_with_source("Failed to encode a Python template value.", error)
    })?;
    Ok(RUN_TEMPLATE.replace("{{CONFIG_JSON}}", &encoded))
}
