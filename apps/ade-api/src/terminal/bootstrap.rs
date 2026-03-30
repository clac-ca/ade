use serde::Serialize;

use crate::error::AppError;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalBootstrapConfig {
    pub(crate) bridge_url: String,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

const BOOTSTRAP_TEMPLATE: &str = include_str!("bootstrap.py.tmpl");

pub(crate) fn render_bootstrap_code(config: &TerminalBootstrapConfig) -> Result<String, AppError> {
    if !BOOTSTRAP_TEMPLATE.contains("{{CONFIG_JSON}}") {
        return Err(AppError::internal(
            "Terminal bootstrap template is missing the CONFIG_JSON placeholder.",
        ));
    }

    let config_json = serde_json::to_string(config).map_err(|error| {
        AppError::internal_with_source(
            "Failed to encode the terminal bootstrap configuration.",
            error,
        )
    })?;

    let encoded = serde_json::to_string(&config_json).map_err(|error| {
        AppError::internal_with_source(
            "Failed to encode the terminal bootstrap JSON string.",
            error,
        )
    })?;

    Ok(BOOTSTRAP_TEMPLATE.replace("{{CONFIG_JSON}}", &encoded))
}
