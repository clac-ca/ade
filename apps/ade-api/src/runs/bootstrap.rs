use std::collections::BTreeMap;

use reqwest::Url;
use serde::Serialize;

use crate::{
    artifacts::{ArtifactAccessGrant, resolve_access_url},
    error::AppError,
};

const BOOTSTRAP_TEMPLATE: &str = include_str!("bootstrap.py.tmpl");

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootstrapArtifactAccess {
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) method: String,
    pub(crate) url: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunBootstrapConfig {
    pub(crate) config_package_name: &'static str,
    pub(crate) config_version: String,
    pub(crate) config_wheel_path: String,
    pub(crate) bridge_url: String,
    pub(crate) engine_package_name: &'static str,
    pub(crate) engine_version: String,
    pub(crate) engine_wheel_path: String,
    pub(crate) input_download: BootstrapArtifactAccess,
    pub(crate) install_lock_path: String,
    pub(crate) local_input_path: String,
    pub(crate) local_output_dir: String,
    pub(crate) output_path: String,
    pub(crate) output_upload: BootstrapArtifactAccess,
}

pub(crate) fn bootstrap_artifact_access(
    app_url: &Url,
    access: ArtifactAccessGrant,
) -> Result<BootstrapArtifactAccess, AppError> {
    let resolved_url = resolve_access_url(app_url, &access)?;
    Ok(BootstrapArtifactAccess {
        headers: access.headers,
        method: access.method.to_string(),
        url: resolved_url,
    })
}

pub(crate) fn render_bootstrap_code(config: &RunBootstrapConfig) -> Result<String, AppError> {
    if !BOOTSTRAP_TEMPLATE.contains("{{CONFIG_JSON}}") {
        return Err(AppError::internal(
            "Run bootstrap template is missing the CONFIG_JSON placeholder.",
        ));
    }

    let config_json = serde_json::to_string(config).map_err(|error| {
        AppError::internal_with_source("Failed to encode the run bootstrap configuration.", error)
    })?;
    let encoded = serde_json::to_string(&config_json).map_err(|error| {
        AppError::internal_with_source("Failed to encode the run bootstrap JSON string.", error)
    })?;
    Ok(BOOTSTRAP_TEMPLATE.replace("{{CONFIG_JSON}}", &encoded))
}

pub(crate) fn local_input_path(input_path: &str) -> String {
    let filename = uploaded_filename(input_path).unwrap_or_else(|_| "input.bin".to_string());
    format!("/mnt/data/work/input/{filename}")
}

pub(crate) fn local_output_dir(run_id: &str) -> String {
    format!("/mnt/data/runs/{run_id}/output")
}

pub(crate) fn session_path(relative_path: &str) -> String {
    format!("/mnt/data/{}", relative_path.trim_start_matches('/'))
}

fn uploaded_filename(filename: &str) -> Result<String, AppError> {
    let name = std::path::Path::new(filename.trim())
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::request("Uploaded file must include a valid filename."))?;

    if name.is_empty() {
        return Err(AppError::request(
            "Uploaded file must include a valid filename.",
        ));
    }

    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        BootstrapArtifactAccess, RunBootstrapConfig, local_input_path, render_bootstrap_code,
        uploaded_filename,
    };

    #[test]
    fn bootstrap_code_contains_direct_access_and_public_output_path() {
        let code = render_bootstrap_code(&RunBootstrapConfig {
            config_package_name: "ade-config",
            config_version: "1.0.0".to_string(),
            config_wheel_path: "/mnt/data/config.whl".to_string(),
            bridge_url: "wss://example.com/bridge".to_string(),
            engine_package_name: "ade-engine",
            engine_version: "1.0.0".to_string(),
            engine_wheel_path: "/mnt/data/engine.whl".to_string(),
            input_download: BootstrapArtifactAccess {
                headers: BTreeMap::new(),
                method: "GET".to_string(),
                url: "https://example.com/input".to_string(),
            },
            install_lock_path: "/mnt/data/.lock".to_string(),
            local_input_path: "/mnt/data/work/input/input.xlsx".to_string(),
            local_output_dir: "/mnt/data/runs/run-1/output".to_string(),
            output_path:
                "workspaces/workspace-a/configs/config-v1/runs/run-1/output/normalized.xlsx"
                    .to_string(),
            output_upload: BootstrapArtifactAccess {
                headers: BTreeMap::new(),
                method: "PUT".to_string(),
                url: "https://example.com/output".to_string(),
            },
        })
        .unwrap();

        assert!(code.contains("urllib.request"));
        assert!(code.contains("normalized.xlsx"));
        assert!(code.contains("https://example.com/input"));
    }

    #[test]
    fn local_input_paths_use_safe_basenames() {
        assert_eq!(
            local_input_path("workspaces/a/configs/b/uploads/u/input.xlsx"),
            "/mnt/data/work/input/input.xlsx"
        );
    }

    #[test]
    fn uploaded_filenames_are_reduced_to_a_safe_basename() {
        assert_eq!(uploaded_filename("/tmp/input.xlsx").unwrap(), "input.xlsx");
    }
}
