use std::{collections::BTreeMap, path::PathBuf};

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::error::AppError;

use super::{
    ArtifactAccessGrant, ArtifactMetadata, ArtifactStore, LOCAL_ARTIFACT_TOKEN_HEADER,
    artifact_content_type, artifact_url_path, format_iso_8601, local_artifact_token,
    normalize_artifact_path,
};

pub(super) struct FileSystemArtifactStore {
    root: PathBuf,
    token_secret: String,
}

impl FileSystemArtifactStore {
    pub(super) fn new(root: PathBuf, token_secret: String) -> Self {
        Self { root, token_secret }
    }

    fn absolute_path(&self, path: &str) -> Result<PathBuf, AppError> {
        Ok(self.root.join(normalize_artifact_path(path)?))
    }

    fn access_headers(
        &self,
        path: &str,
        method: &'static str,
        expires_at: OffsetDateTime,
    ) -> Result<BTreeMap<String, String>, AppError> {
        let mut headers = BTreeMap::new();
        headers.insert(
            LOCAL_ARTIFACT_TOKEN_HEADER.to_string(),
            local_artifact_token(&self.token_secret, method, path, expires_at)?,
        );
        Ok(headers)
    }
}

#[async_trait]
impl ArtifactStore for FileSystemArtifactStore {
    async fn create_download_access(
        &self,
        path: &str,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        Ok(ArtifactAccessGrant {
            expires_at: format_iso_8601(expires_at)?,
            headers: self.access_headers(path, "GET", expires_at)?,
            method: "GET",
            url: artifact_url_path(path)?,
        })
    }

    async fn create_upload_access(
        &self,
        path: &str,
        content_type: Option<&str>,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        let mut headers = self.access_headers(path, "PUT", expires_at)?;
        headers.insert(
            reqwest::header::CONTENT_TYPE.as_str().to_string(),
            artifact_content_type(path, content_type),
        );
        Ok(ArtifactAccessGrant {
            expires_at: format_iso_8601(expires_at)?,
            headers,
            method: "PUT",
            url: artifact_url_path(path)?,
        })
    }

    async fn download_bytes(&self, path: &str) -> Result<(String, Vec<u8>), AppError> {
        let normalized = normalize_artifact_path(path)?;
        let absolute_path = self.absolute_path(&normalized)?;
        let content = tokio::fs::read(&absolute_path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::not_found("Artifact not found.")
            } else {
                AppError::io_with_source(
                    format!("Failed to read '{}'.", absolute_path.display()),
                    error,
                )
            }
        })?;
        Ok((artifact_content_type(&normalized, None), content))
    }

    async fn metadata(&self, path: &str) -> Result<Option<ArtifactMetadata>, AppError> {
        let normalized = normalize_artifact_path(path)?;
        let absolute_path = self.absolute_path(&normalized)?;
        let metadata = match tokio::fs::metadata(&absolute_path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(AppError::io_with_source(
                    format!("Failed to read metadata for '{}'.", absolute_path.display()),
                    error,
                ));
            }
        };

        Ok(Some(ArtifactMetadata {
            content_type: artifact_content_type(&normalized, None),
            size: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
        }))
    }

    async fn upload_bytes(
        &self,
        path: &str,
        content_type: Option<&str>,
        content: Vec<u8>,
    ) -> Result<ArtifactMetadata, AppError> {
        let normalized = normalize_artifact_path(path)?;
        let absolute_path = self.absolute_path(&normalized)?;

        if let Some(parent) = absolute_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                AppError::io_with_source(format!("Failed to create '{}'.", parent.display()), error)
            })?;
        }

        tokio::fs::write(&absolute_path, &content)
            .await
            .map_err(|error| {
                AppError::io_with_source(
                    format!("Failed to write '{}'.", absolute_path.display()),
                    error,
                )
            })?;

        Ok(ArtifactMetadata {
            content_type: artifact_content_type(&normalized, content_type),
            size: content.len(),
        })
    }
}
