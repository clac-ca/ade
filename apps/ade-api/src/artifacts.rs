use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::{Method, Url};
use sha2::Sha256;
use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
};

mod blob;
mod filesystem;

use blob::BlobArtifactStore;
use filesystem::FileSystemArtifactStore;

pub(super) const ARTIFACTS_ROOT_ENV_NAME: &str = "ADE_ARTIFACTS_ROOT";
pub(super) const BLOB_ACCOUNT_KEY_ENV_NAME: &str = "ADE_BLOB_ACCOUNT_KEY";
pub(super) const BLOB_ACCOUNT_URL_ENV_NAME: &str = "ADE_BLOB_ACCOUNT_URL";
pub(super) const BLOB_CONTAINER_ENV_NAME: &str = "ADE_BLOB_CONTAINER";
pub(super) const BLOB_CORS_ALLOWED_ORIGINS_ENV_NAME: &str = "ADE_BLOB_CORS_ALLOWED_ORIGINS";
pub(super) const BLOB_PUBLIC_ACCOUNT_URL_ENV_NAME: &str = "ADE_BLOB_PUBLIC_ACCOUNT_URL";
pub(super) const BLOB_RUNTIME_ACCOUNT_URL_ENV_NAME: &str = "ADE_BLOB_RUNTIME_ACCOUNT_URL";
pub(super) const LOCAL_ARTIFACT_TOKEN_HEADER: &str = "x-ade-artifact-token";
const LOCAL_ARTIFACT_URL_BASE: &str = "http://local.invalid";

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

#[async_trait]
pub(crate) trait ArtifactStore: Send + Sync {
    async fn create_browser_upload_access(
        &self,
        path: &str,
        content_type: Option<&str>,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        self.create_upload_access(path, content_type, expires_at)
            .await
    }

    async fn create_download_access(
        &self,
        path: &str,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError>;

    async fn create_upload_access(
        &self,
        path: &str,
        content_type: Option<&str>,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError>;

    async fn download_bytes(&self, path: &str) -> Result<(String, Vec<u8>), AppError>;

    async fn metadata(&self, path: &str) -> Result<Option<ArtifactMetadata>, AppError>;

    async fn upload_bytes(
        &self,
        path: &str,
        content_type: Option<&str>,
        content: Vec<u8>,
    ) -> Result<ArtifactMetadata, AppError>;
}

pub(crate) type ArtifactStoreHandle = Arc<dyn ArtifactStore>;

pub(crate) fn artifact_store_from_env(
    env: &EnvBag,
    local_token_secret: &str,
) -> Result<ArtifactStoreHandle, AppError> {
    let account_key = read_optional_trimmed_string(env, BLOB_ACCOUNT_KEY_ENV_NAME);
    let account_url = read_optional_trimmed_string(env, BLOB_ACCOUNT_URL_ENV_NAME);
    let container = read_optional_trimmed_string(env, BLOB_CONTAINER_ENV_NAME);
    let public_account_url = read_optional_trimmed_string(env, BLOB_PUBLIC_ACCOUNT_URL_ENV_NAME);
    let runtime_account_url = read_optional_trimmed_string(env, BLOB_RUNTIME_ACCOUNT_URL_ENV_NAME);
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

    match (account_url, container, account_key) {
        (None, None, None) => {
            let root = read_optional_trimmed_string(env, ARTIFACTS_ROOT_ENV_NAME)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".ade-artifacts"));
            Ok(Arc::new(FileSystemArtifactStore::new(
                root,
                local_token_secret.to_string(),
            )))
        }
        (Some(account_url), Some(container), account_key) => Ok(Arc::new(BlobArtifactStore::new(
            account_url,
            public_account_url,
            runtime_account_url,
            container,
            account_key,
            cors_allowed_origins,
        )?)),
        (None, None, Some(_)) | (Some(_), None, _) | (None, Some(_), _) => {
            Err(AppError::config(format!(
                "Configure {BLOB_ACCOUNT_URL_ENV_NAME} and {BLOB_CONTAINER_ENV_NAME} together."
            )))
        }
    }
}

pub(crate) fn upload_id() -> String {
    format!("upl_{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn output_path_for_run(scope: &Scope, run_id: &str) -> String {
    format!("{}/runs/{run_id}/output/normalized.xlsx", scope_root(scope))
}

pub(crate) fn scope_root(scope: &Scope) -> String {
    format!(
        "workspaces/{}/configs/{}",
        sanitize_scope_segment(&scope.workspace_id),
        sanitize_scope_segment(&scope.config_version_id)
    )
}

pub(crate) fn upload_path_for_file(scope: &Scope, upload_id: &str, filename: &str) -> String {
    format!(
        "{}/uploads/{upload_id}/{}",
        scope_root(scope),
        normalize_filename(filename)
    )
}

pub(crate) fn validate_input_path(scope: &Scope, path: &str) -> Result<String, AppError> {
    let normalized = normalize_artifact_path(path)?;
    let prefix = format!("{}/uploads/", scope_root(scope));
    if normalized.starts_with(&prefix) {
        return Ok(normalized);
    }

    Err(AppError::request(
        "Run inputPath must reference a scoped upload created by POST /uploads.",
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

pub(crate) fn local_artifact_token_header() -> &'static str {
    LOCAL_ARTIFACT_TOKEN_HEADER
}

pub(crate) fn resolve_access_url(
    base_url: &Url,
    access: &ArtifactAccessGrant,
) -> Result<String, AppError> {
    match Url::parse(&access.url) {
        Ok(url) => Ok(url.to_string()),
        Err(_) => base_url
            .join(&access.url)
            .map(|url| url.to_string())
            .map_err(|error| {
                AppError::internal_with_source("Failed to resolve an artifact access URL.", error)
            }),
    }
}

pub(crate) fn verify_local_artifact_access(
    path: &str,
    method: &Method,
    token: &str,
    secret: &str,
    now: OffsetDateTime,
) -> Result<String, AppError> {
    let normalized = normalize_artifact_path(path)?;
    let Some((expires_at_timestamp, signature_hex)) = token.split_once('.') else {
        return Err(AppError::status(
            reqwest::StatusCode::UNAUTHORIZED,
            "Invalid artifact access token.",
        ));
    };
    let expires_at_timestamp = expires_at_timestamp.parse::<i64>().map_err(|_| {
        AppError::status(
            reqwest::StatusCode::UNAUTHORIZED,
            "Invalid artifact access token.",
        )
    })?;

    if now.unix_timestamp() > expires_at_timestamp {
        return Err(AppError::status(
            reqwest::StatusCode::UNAUTHORIZED,
            "Artifact access token expired.",
        ));
    }

    let signature = hex::decode(signature_hex).map_err(|_| {
        AppError::status(
            reqwest::StatusCode::UNAUTHORIZED,
            "Invalid artifact access token.",
        )
    })?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is always valid");
    mac.update(method.as_str().as_bytes());
    mac.update(b":");
    mac.update(normalized.as_bytes());
    mac.update(b":");
    mac.update(expires_at_timestamp.to_string().as_bytes());
    mac.verify_slice(&signature).map_err(|_| {
        AppError::status(
            reqwest::StatusCode::UNAUTHORIZED,
            "Invalid artifact access token.",
        )
    })?;

    Ok(normalized)
}

pub(super) fn artifact_content_type(path: &str, content_type: Option<&str>) -> String {
    content_type.map(ToOwned::to_owned).unwrap_or_else(|| {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string()
    })
}

pub(super) fn artifact_url_path(path: &str) -> Result<String, AppError> {
    let normalized = normalize_artifact_path(path)?;
    let mut url = Url::parse(LOCAL_ARTIFACT_URL_BASE).expect("local artifact base URL is valid");
    {
        let mut segments = url.path_segments_mut().map_err(|()| {
            AppError::internal("Local artifact route base URL is not a valid path base.")
        })?;
        segments.pop_if_empty();
        segments.extend(["api", "internal", "artifacts"]);
        for segment in normalized.split('/') {
            segments.push(segment);
        }
    }
    Ok(url.path().to_string())
}

pub(super) fn format_iso_8601(value: OffsetDateTime) -> Result<String, AppError> {
    value.format(ISO_8601_SECONDS_FORMAT).map_err(|error| {
        AppError::internal_with_source("Failed to format an ISO-8601 timestamp.", error)
    })
}

pub(super) fn local_artifact_token(
    secret: &str,
    method: &str,
    path: &str,
    expires_at: OffsetDateTime,
) -> Result<String, AppError> {
    let normalized = normalize_artifact_path(path)?;
    let expires_at_timestamp = expires_at.unix_timestamp();
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is always valid");
    mac.update(method.as_bytes());
    mac.update(b":");
    mac.update(normalized.as_bytes());
    mac.update(b":");
    mac.update(expires_at_timestamp.to_string().as_bytes());
    Ok(format!(
        "{expires_at_timestamp}.{}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

fn normalize_filename(filename: &str) -> String {
    Path::new(filename.trim())
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("upload.bin")
        .to_string()
}

fn sanitize_scope_segment(value: &str) -> String {
    value.trim_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use reqwest::Method;
    use time::{Duration as TimeDuration, OffsetDateTime};

    use super::{
        normalize_artifact_path, output_path_for_run, scope_root, upload_path_for_file,
        validate_input_path, verify_local_artifact_access,
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
            upload_path_for_file(&scope, "upl_123", "/tmp/input.xlsx"),
            "workspaces/workspace-a/configs/config-v1/uploads/upl_123/input.xlsx"
        );
        assert_eq!(
            output_path_for_run(&scope, "run_123"),
            "workspaces/workspace-a/configs/config-v1/runs/run_123/output/normalized.xlsx"
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

    #[test]
    fn local_artifact_tokens_bind_method_and_path() {
        let expires_at = OffsetDateTime::now_utc() + TimeDuration::minutes(5);
        let token = super::local_artifact_token(
            "secret",
            "PUT",
            "workspaces/a/configs/b/uploads/u/input.xlsx",
            expires_at,
        )
        .unwrap();

        assert!(
            verify_local_artifact_access(
                "workspaces/a/configs/b/uploads/u/input.xlsx",
                &Method::PUT,
                &token,
                "secret",
                OffsetDateTime::now_utc(),
            )
            .is_ok()
        );
        assert!(
            verify_local_artifact_access(
                "workspaces/a/configs/b/uploads/u/input.xlsx",
                &Method::GET,
                &token,
                "secret",
                OffsetDateTime::now_utc(),
            )
            .is_err()
        );
    }
}
