use std::{collections::BTreeMap, path::Path};

use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};

use crate::{
    azure_auth::read_azure_client_id,
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
};

mod blob;
pub(crate) use blob::BlobArtifactStore;

pub(super) const BLOB_ACCOUNT_KEY_ENV_NAME: &str = "ADE_BLOB_ACCOUNT_KEY";
pub(super) const BLOB_ACCOUNT_URL_ENV_NAME: &str = "ADE_BLOB_ACCOUNT_URL";
pub(super) const BLOB_CONTAINER_ENV_NAME: &str = "ADE_BLOB_CONTAINER";
pub(super) const BLOB_CORS_ALLOWED_ORIGINS_ENV_NAME: &str = "ADE_BLOB_CORS_ALLOWED_ORIGINS";
pub(super) const BLOB_PUBLIC_ACCOUNT_URL_ENV_NAME: &str = "ADE_BLOB_PUBLIC_ACCOUNT_URL";

static ISO_8601_SECONDS_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactAccessGrant {
    pub(crate) expires_at: String,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) method: &'static str,
    pub(crate) url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactMetadata {
    pub(crate) content_type: String,
    pub(crate) size: usize,
}

pub(crate) fn blob_artifact_store_from_env(env: &EnvBag) -> Result<BlobArtifactStore, AppError> {
    let account_key = read_optional_trimmed_string(env, BLOB_ACCOUNT_KEY_ENV_NAME);
    let account_url =
        read_optional_trimmed_string(env, BLOB_ACCOUNT_URL_ENV_NAME).ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {BLOB_ACCOUNT_URL_ENV_NAME}"
            ))
        })?;
    let container =
        read_optional_trimmed_string(env, BLOB_CONTAINER_ENV_NAME).ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {BLOB_CONTAINER_ENV_NAME}"
            ))
        })?;
    let public_account_url = read_optional_trimmed_string(env, BLOB_PUBLIC_ACCOUNT_URL_ENV_NAME);
    let cors_allowed_origins =
        read_optional_trimmed_string(env, BLOB_CORS_ALLOWED_ORIGINS_ENV_NAME)
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

    BlobArtifactStore::new(
        account_url,
        public_account_url,
        container,
        account_key,
        cors_allowed_origins,
        read_azure_client_id(env),
    )
}

pub(crate) fn upload_id() -> String {
    format!("upl_{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn upload_batch_id() -> String {
    format!("bat_{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn upload_file_id() -> String {
    format!("fil_{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn output_path_for_run(scope: &Scope, run_id: &str) -> String {
    format!("{}/runs/{run_id}/output/normalized.xlsx", scope_root(scope))
}

pub(crate) fn log_path_for_run(scope: &Scope, run_id: &str) -> String {
    format!("{}/runs/{run_id}/logs/events.ndjson", scope_root(scope))
}

pub(crate) fn scope_root(scope: &Scope) -> String {
    format!(
        "workspaces/{}/configs/{}",
        sanitize_scope_segment(&scope.workspace_id),
        sanitize_scope_segment(&scope.config_version_id)
    )
}

pub(crate) fn upload_path_for_file(
    scope: &Scope,
    upload_id: &str,
    filename: &str,
) -> Result<String, AppError> {
    let filename = validated_upload_filename(filename)?;
    Ok(format!(
        "{}/uploads/{upload_id}/{}",
        scope_root(scope),
        filename
    ))
}

pub(crate) fn upload_path_for_batch_file(
    scope: &Scope,
    batch_id: &str,
    file_id: &str,
    filename: &str,
) -> Result<String, AppError> {
    let filename = validated_upload_filename(filename)?;
    Ok(format!(
        "{}/uploads/batches/{batch_id}/{file_id}/{filename}",
        scope_root(scope)
    ))
}

pub(crate) fn validate_input_path(scope: &Scope, path: &str) -> Result<String, AppError> {
    let normalized = normalize_artifact_path(path)?;
    let prefix = format!("{}/uploads/", scope_root(scope));
    if normalized.starts_with(&prefix) {
        return Ok(normalized);
    }

    Err(AppError::request(
        "Run inputPath must reference a scoped upload created by POST /uploads or POST /uploads/batches.",
    ))
}

pub(super) fn normalize_artifact_path(path: &str) -> Result<String, AppError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::request(
            "Artifact path must be a non-empty relative path.",
        ));
    }

    let mut segments = Vec::new();
    for component in Path::new(trimmed).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(segment) => {
                segments.push(segment.to_string_lossy().to_string());
            }
            std::path::Component::ParentDir
            | std::path::Component::Prefix(_)
            | std::path::Component::RootDir => {
                return Err(AppError::request("Artifact path must be a relative path."));
            }
        }
    }

    if segments.is_empty() {
        return Err(AppError::request(
            "Artifact path must be a non-empty relative path.",
        ));
    }

    Ok(segments.join("/"))
}

pub(super) fn artifact_content_type(path: &str, content_type: Option<&str>) -> String {
    content_type.map(ToOwned::to_owned).unwrap_or_else(|| {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string()
    })
}

pub(super) fn format_iso_8601(value: OffsetDateTime) -> Result<String, AppError> {
    value.format(ISO_8601_SECONDS_FORMAT).map_err(|error| {
        AppError::internal_with_source("Failed to format an ISO-8601 timestamp.", error)
    })
}

fn validated_upload_filename(filename: &str) -> Result<String, AppError> {
    Path::new(filename.trim())
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::request("Upload filename must resolve to a non-empty basename."))
}

fn sanitize_scope_segment(value: &str) -> String {
    value.trim_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        log_path_for_run, normalize_artifact_path, output_path_for_run, scope_root,
        upload_path_for_batch_file, upload_path_for_file, validate_input_path,
    };
    use crate::scope::Scope;

    #[test]
    fn normalizes_relative_artifact_paths() {
        assert_eq!(
            normalize_artifact_path("./workspaces/a/configs/b/uploads/u/input.xlsx").unwrap(),
            "workspaces/a/configs/b/uploads/u/input.xlsx"
        );
        assert!(normalize_artifact_path("../etc/passwd").is_err());
        assert!(normalize_artifact_path("/tmp").is_err());
    }

    #[test]
    fn scoped_paths_are_predictable() {
        let scope = Scope {
            workspace_id: "workspace-a".to_string(),
            config_version_id: "config-v1".to_string(),
        };
        assert_eq!(
            scope_root(&scope),
            "workspaces/workspace-a/configs/config-v1"
        );
        assert_eq!(
            upload_path_for_file(&scope, "upl_123", "/tmp/input.xlsx").unwrap(),
            "workspaces/workspace-a/configs/config-v1/uploads/upl_123/input.xlsx"
        );
        assert_eq!(
            upload_path_for_batch_file(&scope, "bat_123", "fil_123", "/tmp/bulk/input.xlsx")
                .unwrap(),
            "workspaces/workspace-a/configs/config-v1/uploads/batches/bat_123/fil_123/input.xlsx"
        );
        assert_eq!(
            output_path_for_run(&scope, "run_123"),
            "workspaces/workspace-a/configs/config-v1/runs/run_123/output/normalized.xlsx"
        );
        assert_eq!(
            log_path_for_run(&scope, "run_123"),
            "workspaces/workspace-a/configs/config-v1/runs/run_123/logs/events.ndjson"
        );
    }

    #[test]
    fn validate_input_path_restricts_runs_to_scoped_uploads() {
        let scope = Scope {
            workspace_id: "workspace-a".to_string(),
            config_version_id: "config-v1".to_string(),
        };
        assert!(
            validate_input_path(
                &scope,
                "workspaces/workspace-a/configs/config-v1/uploads/upl_123/input.xlsx"
            )
            .is_ok()
        );
        assert!(
            validate_input_path(
                &scope,
                "workspaces/workspace-a/configs/config-v1/runs/run_123/output/normalized.xlsx"
            )
            .is_err()
        );
    }
}
