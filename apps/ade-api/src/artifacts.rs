use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use azure_core::credentials::TokenCredential;
use azure_identity::{DeveloperToolsCredential, ManagedIdentityCredential};
use quick_xml::de::from_str as xml_from_str;
use reqwest::{
    Client, Method, StatusCode, Url,
    header::{CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::Deserialize;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    session::{Scope, SessionFile},
};

const ARTIFACTS_ROOT_ENV_NAME: &str = "ADE_ARTIFACTS_ROOT";
const BLOB_ACCOUNT_URL_ENV_NAME: &str = "ADE_BLOB_ACCOUNT_URL";
const BLOB_CONTAINER_ENV_NAME: &str = "ADE_BLOB_CONTAINER";
const STORAGE_AUDIENCE_SCOPE: &str = "https://storage.azure.com/.default";
const STORAGE_SERVICE_VERSION: &str = "2024-11-04";

#[async_trait]
pub(crate) trait ArtifactStore: Send + Sync {
    async fn download(&self, scope: &Scope, path: &str) -> Result<(String, Vec<u8>), AppError>;
    async fn list(&self, scope: &Scope) -> Result<Vec<SessionFile>, AppError>;
    async fn upload(
        &self,
        scope: &Scope,
        path: &str,
        content_type: Option<&str>,
        content: Vec<u8>,
    ) -> Result<SessionFile, AppError>;
}

pub(crate) type ArtifactStoreHandle = Arc<dyn ArtifactStore>;

pub(crate) fn artifact_store_from_env(env: &EnvBag) -> Result<ArtifactStoreHandle, AppError> {
    let account_url = read_optional_trimmed_string(env, BLOB_ACCOUNT_URL_ENV_NAME);
    let container = read_optional_trimmed_string(env, BLOB_CONTAINER_ENV_NAME);

    match (account_url, container) {
        (Some(account_url), Some(container)) => {
            Ok(Arc::new(BlobArtifactStore::new(account_url, container)?))
        }
        (None, None) => {
            let root = read_optional_trimmed_string(env, ARTIFACTS_ROOT_ENV_NAME)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".ade-artifacts"));
            Ok(Arc::new(FileSystemArtifactStore::new(root)))
        }
        _ => Err(AppError::config(format!(
            "Configure both {BLOB_ACCOUNT_URL_ENV_NAME} and {BLOB_CONTAINER_ENV_NAME}, or neither."
        ))),
    }
}

pub(crate) fn normalize_artifact_path(path: &str) -> Result<String, AppError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::request(
            "Session file path must be a non-empty relative path.",
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
                return Err(AppError::request(
                    "Session file path must be a relative path inside the session.",
                ));
            }
        }
    }

    if segments.is_empty() {
        return Err(AppError::request(
            "Session file path must be a non-empty relative path.",
        ));
    }

    Ok(segments.join("/"))
}

fn artifact_content_type(path: &str, content_type: Option<&str>) -> String {
    content_type.map(ToOwned::to_owned).unwrap_or_else(|| {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string()
    })
}

fn artifact_scope_prefix(scope: &Scope) -> String {
    format!(
        "{}/{}",
        hex::encode(scope.workspace_id.as_bytes()),
        hex::encode(scope.config_version_id.as_bytes())
    )
}

fn artifact_storage_key(scope: &Scope, path: &str) -> String {
    format!("{}/{}", artifact_scope_prefix(scope), path)
}

struct FileSystemArtifactStore {
    root: PathBuf,
}

impl FileSystemArtifactStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn scope_root(&self, scope: &Scope) -> PathBuf {
        self.root.join(artifact_scope_prefix(scope))
    }

    fn absolute_path(&self, scope: &Scope, path: &str) -> PathBuf {
        self.scope_root(scope).join(path)
    }
}

#[async_trait]
impl ArtifactStore for FileSystemArtifactStore {
    async fn download(&self, scope: &Scope, path: &str) -> Result<(String, Vec<u8>), AppError> {
        let normalized = normalize_artifact_path(path)?;
        let absolute_path = self.absolute_path(scope, &normalized);
        let content = tokio::fs::read(&absolute_path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::not_found("Session file not found.")
            } else {
                AppError::io_with_source(
                    format!("Failed to read '{}'.", absolute_path.display()),
                    error,
                )
            }
        })?;
        Ok((artifact_content_type(&normalized, None), content))
    }

    async fn list(&self, scope: &Scope) -> Result<Vec<SessionFile>, AppError> {
        let scope_root = self.scope_root(scope);
        let mut files = Vec::new();
        collect_files(&scope_root, &scope_root, &mut files).await?;
        files.sort_by(|left, right| left.filename.cmp(&right.filename));
        Ok(files)
    }

    async fn upload(
        &self,
        scope: &Scope,
        path: &str,
        _content_type: Option<&str>,
        content: Vec<u8>,
    ) -> Result<SessionFile, AppError> {
        let normalized = normalize_artifact_path(path)?;
        let absolute_path = self.absolute_path(scope, &normalized);

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

        Ok(SessionFile {
            filename: normalized.clone(),
            last_modified_time: None,
            size: content.len(),
        })
    }
}

async fn collect_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<SessionFile>,
) -> Result<(), AppError> {
    let mut queue = VecDeque::from([current.to_path_buf()]);

    while let Some(directory) = queue.pop_front() {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(AppError::io_with_source(
                    format!("Failed to read '{}'.", directory.display()),
                    error,
                ));
            }
        };

        while let Some(entry) = entries.next_entry().await.map_err(|error| {
            AppError::io_with_source(
                format!("Failed to enumerate '{}'.", directory.display()),
                error,
            )
        })? {
            let entry_path = entry.path();
            let file_type = entry.file_type().await.map_err(|error| {
                AppError::io_with_source(
                    format!("Failed to inspect '{}'.", entry_path.display()),
                    error,
                )
            })?;

            if file_type.is_dir() {
                queue.push_back(entry_path);
                continue;
            }

            let metadata = entry.metadata().await.map_err(|error| {
                AppError::io_with_source(
                    format!("Failed to read metadata for '{}'.", entry_path.display()),
                    error,
                )
            })?;
            let relative_path = entry_path
                .strip_prefix(root)
                .unwrap_or(entry_path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            files.push(SessionFile {
                filename: relative_path,
                last_modified_time: None,
                size: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            });
        }
    }

    Ok(())
}

struct BlobArtifactStore {
    account_url: Url,
    client: Client,
    container: String,
}

impl BlobArtifactStore {
    fn new(account_url: String, container: String) -> Result<Self, AppError> {
        let account_url = Url::parse(&account_url).map_err(|error| {
            AppError::config_with_source(
                format!("{BLOB_ACCOUNT_URL_ENV_NAME} is not a valid URL."),
                error,
            )
        })?;
        Ok(Self {
            account_url,
            client: Client::new(),
            container,
        })
    }

    fn blob_url(&self, key: &str) -> Result<Url, AppError> {
        let mut url = self.account_url.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|()| {
                AppError::config("Blob account URL cannot be used as a base URL.".to_string())
            })?;
            segments.pop_if_empty();
            segments.push(&self.container);
            for segment in key.split('/') {
                segments.push(segment);
            }
        }
        Ok(url)
    }

    fn list_url(&self, prefix: &str) -> Result<Url, AppError> {
        let mut url = self.account_url.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|()| {
                AppError::config("Blob account URL cannot be used as a base URL.".to_string())
            })?;
            segments.pop_if_empty();
            segments.push(&self.container);
        }
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("restype", "container");
            query.append_pair("comp", "list");
            query.append_pair("prefix", prefix);
        }
        Ok(url)
    }

    async fn request(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, AppError> {
        let token = blob_access_token().await?;
        let mut builder = self.client.request(method, url).bearer_auth(token);
        builder = builder.headers(headers);
        if let Some(body) = body {
            builder = builder.body(body);
        }
        let response = builder.send().await.map_err(|error| {
            AppError::internal_with_source("Failed to call Azure Blob Storage.".to_string(), error)
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let body = response.text().await.unwrap_or_default();
        Err(match status {
            StatusCode::NOT_FOUND => AppError::not_found("Session file not found."),
            StatusCode::CONFLICT | StatusCode::BAD_REQUEST => AppError::request(body),
            _ => AppError::status(status, body),
        })
    }
}

#[async_trait]
impl ArtifactStore for BlobArtifactStore {
    async fn download(&self, scope: &Scope, path: &str) -> Result<(String, Vec<u8>), AppError> {
        let normalized = normalize_artifact_path(path)?;
        let key = artifact_storage_key(scope, &normalized);
        let response = self
            .request(
                Method::GET,
                self.blob_url(&key)?,
                blob_request_headers(None)?,
                None,
            )
            .await?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let body = response.bytes().await.map_err(|error| {
            AppError::internal_with_source(
                "Failed to read the Azure Blob Storage response.".to_string(),
                error,
            )
        })?;
        Ok((content_type, body.to_vec()))
    }

    async fn list(&self, scope: &Scope) -> Result<Vec<SessionFile>, AppError> {
        let prefix = format!("{}/", artifact_scope_prefix(scope));
        let response = self
            .request(
                Method::GET,
                self.list_url(&prefix)?,
                blob_request_headers(None)?,
                None,
            )
            .await?;
        let xml = response.text().await.map_err(|error| {
            AppError::internal_with_source(
                "Failed to read the Azure Blob Storage listing.".to_string(),
                error,
            )
        })?;
        let listing = xml_from_str::<BlobListResponse>(&xml).map_err(|error| {
            AppError::internal_with_source(
                "Failed to decode the Azure Blob Storage listing.".to_string(),
                error,
            )
        })?;
        let mut files = listing
            .blobs
            .map(|blobs| {
                blobs
                    .items
                    .into_iter()
                    .filter_map(|blob| {
                        blob.name
                            .strip_prefix(&prefix)
                            .map(|relative_path| SessionFile {
                                filename: relative_path.to_string(),
                                last_modified_time: blob.properties.last_modified,
                                size: blob.properties.content_length,
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        files.sort_by(|left, right| left.filename.cmp(&right.filename));
        Ok(files)
    }

    async fn upload(
        &self,
        scope: &Scope,
        path: &str,
        content_type: Option<&str>,
        content: Vec<u8>,
    ) -> Result<SessionFile, AppError> {
        let normalized = normalize_artifact_path(path)?;
        let key = artifact_storage_key(scope, &normalized);
        let resolved_content_type = artifact_content_type(&normalized, content_type);
        let mut headers = blob_request_headers(Some(&resolved_content_type))?;
        headers.insert("x-ms-blob-type", HeaderValue::from_static("BlockBlob"));
        self.request(
            Method::PUT,
            self.blob_url(&key)?,
            headers,
            Some(content.clone()),
        )
        .await?;
        Ok(SessionFile {
            filename: normalized,
            last_modified_time: None,
            size: content.len(),
        })
    }
}

fn blob_request_headers(content_type: Option<&str>) -> Result<HeaderMap, AppError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-ms-version",
        HeaderValue::from_static(STORAGE_SERVICE_VERSION),
    );
    headers.insert(
        "x-ms-date",
        HeaderValue::from_str(&httpdate::fmt_http_date(std::time::SystemTime::now())).map_err(
            |error| {
                AppError::internal_with_source(
                    "Failed to encode the Azure Blob Storage request date.".to_string(),
                    error,
                )
            },
        )?,
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type.unwrap_or("application/octet-stream")).map_err(
            |error| AppError::request(format!("Invalid artifact content type: {error}")),
        )?,
    );
    Ok(headers)
}

async fn blob_access_token() -> Result<String, AppError> {
    if let Ok(credential) = ManagedIdentityCredential::new(None)
        && let Ok(token) = credential.get_token(&[STORAGE_AUDIENCE_SCOPE], None).await
    {
        return Ok(token.token.secret().to_string());
    }

    let credential = DeveloperToolsCredential::new(None).map_err(|error| {
        AppError::internal_with_source(
            "Failed to create the Azure developer credential.".to_string(),
            error,
        )
    })?;
    let token = credential
        .get_token(&[STORAGE_AUDIENCE_SCOPE], None)
        .await
        .map_err(|error| {
            AppError::internal_with_source(
                "Failed to acquire an Azure access token for Blob Storage.".to_string(),
                error,
            )
        })?;
    Ok(token.token.secret().to_string())
}

#[derive(Deserialize)]
struct BlobListResponse {
    #[serde(rename = "Blobs")]
    blobs: Option<BlobItems>,
}

#[derive(Deserialize)]
struct BlobItems {
    #[serde(rename = "Blob", default)]
    items: Vec<BlobItem>,
}

#[derive(Deserialize)]
struct BlobItem {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties")]
    properties: BlobProperties,
}

#[derive(Deserialize)]
struct BlobProperties {
    #[serde(rename = "Content-Length")]
    content_length: usize,
    #[serde(rename = "Last-Modified")]
    last_modified: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{artifact_scope_prefix, normalize_artifact_path};
    use crate::session::Scope;

    #[test]
    fn normalizes_relative_artifact_paths() {
        assert_eq!(
            normalize_artifact_path("./runs/output.xlsx").unwrap(),
            "runs/output.xlsx"
        );
        assert!(normalize_artifact_path("../etc/passwd").is_err());
        assert!(normalize_artifact_path("/tmp").is_err());
    }

    #[test]
    fn scope_prefixes_are_safe_and_stable() {
        let scope = Scope {
            workspace_id: "workspace-a".to_string(),
            config_version_id: "config-v1".to_string(),
        };
        assert_eq!(
            artifact_scope_prefix(&scope),
            "776f726b73706163652d61/636f6e6669672d7631"
        );
    }
}
