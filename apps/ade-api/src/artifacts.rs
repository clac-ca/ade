use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use azure_core::credentials::TokenCredential;
use azure_identity::{DeveloperToolsCredential, ManagedIdentityCredential};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use hmac::{Hmac, Mac};
use quick_xml::de::from_str as xml_from_str;
use reqwest::{
    Client, Method, StatusCode, Url,
    header::{CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use sha2::Sha256;
use time::{
    Duration as TimeDuration, OffsetDateTime,
    format_description::{FormatItem, well_known::Rfc3339},
    macros::format_description,
};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    session::Scope,
};

const ARTIFACTS_ROOT_ENV_NAME: &str = "ADE_ARTIFACTS_ROOT";
const BLOB_ACCOUNT_URL_ENV_NAME: &str = "ADE_BLOB_ACCOUNT_URL";
const BLOB_CONTAINER_ENV_NAME: &str = "ADE_BLOB_CONTAINER";
const LOCAL_ARTIFACT_TOKEN_HEADER: &str = "x-ade-artifact-token";
const LOCAL_ARTIFACT_URL_BASE: &str = "http://local.invalid";
const SAS_VERSION: &str = "2024-11-04";
const STORAGE_AUDIENCE_SCOPE: &str = "https://storage.azure.com/.default";
const STORAGE_SERVICE_VERSION: &str = "2024-11-04";
const USER_DELEGATION_KEY_REFRESH_BUFFER_MINUTES: i64 = 5;
const USER_DELEGATION_KEY_TTL_MINUTES: i64 = 60;
const USER_DELEGATION_KEY_URL_SUFFIX: &str = "/?restype=service&comp=userdelegationkey";

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
            Ok(Arc::new(FileSystemArtifactStore::new(
                root,
                local_token_secret.to_string(),
            )))
        }
        _ => Err(AppError::config(format!(
            "Configure both {BLOB_ACCOUNT_URL_ENV_NAME} and {BLOB_CONTAINER_ENV_NAME}, or neither."
        ))),
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

pub(crate) fn normalize_artifact_path(path: &str) -> Result<String, AppError> {
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
                return Err(AppError::request(
                    "Artifact path must be a relative path.",
                ));
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

pub(crate) fn resolve_access_url(base_url: &Url, access: &ArtifactAccessGrant) -> Result<String, AppError> {
    match Url::parse(&access.url) {
        Ok(url) => Ok(url.to_string()),
        Err(_) => base_url
            .join(&access.url)
            .map(|url| url.to_string())
            .map_err(|error| {
                AppError::internal_with_source(
                    "Failed to resolve an artifact access URL.",
                    error,
                )
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
            StatusCode::UNAUTHORIZED,
            "Invalid artifact access token.",
        ));
    };
    let expires_at_timestamp = expires_at_timestamp.parse::<i64>().map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid artifact access token.")
    })?;

    if now.unix_timestamp() > expires_at_timestamp {
        return Err(AppError::status(
            StatusCode::UNAUTHORIZED,
            "Artifact access token expired.",
        ));
    }

    let signature = hex::decode(signature_hex).map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid artifact access token.")
    })?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key is always valid");
    mac.update(method.as_str().as_bytes());
    mac.update(b":");
    mac.update(normalized.as_bytes());
    mac.update(b":");
    mac.update(expires_at_timestamp.to_string().as_bytes());
    mac.verify_slice(&signature).map_err(|_| {
        AppError::status(StatusCode::UNAUTHORIZED, "Invalid artifact access token.")
    })?;

    Ok(normalized)
}

fn artifact_content_type(path: &str, content_type: Option<&str>) -> String {
    content_type.map(ToOwned::to_owned).unwrap_or_else(|| {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string()
    })
}

fn artifact_url_path(path: &str) -> Result<String, AppError> {
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

fn format_iso_8601(value: OffsetDateTime) -> Result<String, AppError> {
    value.format(ISO_8601_SECONDS_FORMAT).map_err(|error| {
        AppError::internal_with_source("Failed to format an ISO-8601 timestamp.", error)
    })
}

fn local_artifact_token(
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

struct FileSystemArtifactStore {
    root: PathBuf,
    token_secret: String,
}

impl FileSystemArtifactStore {
    fn new(root: PathBuf, token_secret: String) -> Self {
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
            CONTENT_TYPE.as_str().to_string(),
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
                ))
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

#[derive(Clone)]
struct CachedUserDelegationKey {
    key: UserDelegationKey,
}

struct BlobArtifactStore {
    account_name: String,
    account_url: Url,
    client: Client,
    container: String,
    user_delegation_key: Mutex<Option<CachedUserDelegationKey>>,
}

impl BlobArtifactStore {
    fn new(account_url: String, container: String) -> Result<Self, AppError> {
        let account_url = Url::parse(&account_url).map_err(|error| {
            AppError::config_with_source(
                format!("{BLOB_ACCOUNT_URL_ENV_NAME} is not a valid URL."),
                error,
            )
        })?;
        let account_name = storage_account_name(&account_url)?;

        Ok(Self {
            account_name,
            account_url,
            client: Client::new(),
            container,
            user_delegation_key: Mutex::new(None),
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

    fn canonicalized_resource(&self, key: &str) -> Result<String, AppError> {
        let normalized = normalize_artifact_path(key)?;
        Ok(format!(
            "/blob/{}/{}/{}",
            self.account_name, self.container, normalized
        ))
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

        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(match status {
            StatusCode::NOT_FOUND => AppError::not_found("Artifact not found."),
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT => AppError::request(body),
            _ => AppError::status(status, body),
        })
    }

    async fn request_optional(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
    ) -> Result<Option<reqwest::Response>, AppError> {
        let token = blob_access_token().await?;
        let response = self
            .client
            .request(method, url)
            .bearer_auth(token)
            .headers(headers)
            .send()
            .await
            .map_err(|error| {
                AppError::internal_with_source(
                    "Failed to call Azure Blob Storage.".to_string(),
                    error,
                )
            })?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if response.status().is_success() {
            return Ok(Some(response));
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(match status {
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT => AppError::request(body),
            _ => AppError::status(status, body),
        })
    }

    async fn cached_user_delegation_key(
        &self,
        expires_at: OffsetDateTime,
    ) -> Result<UserDelegationKey, AppError> {
        let minimum_expiry =
            expires_at + TimeDuration::minutes(USER_DELEGATION_KEY_REFRESH_BUFFER_MINUTES);

        if let Some(cached) = self.user_delegation_key.lock().unwrap().clone()
            && cached.key.signed_expiry >= minimum_expiry
        {
            return Ok(cached.key);
        }

        let now = OffsetDateTime::now_utc();
        let signed_start = now - TimeDuration::minutes(5);
        let requested_expiry = expires_at + TimeDuration::minutes(5);
        let signed_expiry = std::cmp::max(
            requested_expiry,
            now + TimeDuration::minutes(USER_DELEGATION_KEY_TTL_MINUTES),
        );
        let key = self
            .fetch_user_delegation_key(signed_start, signed_expiry)
            .await?;

        *self.user_delegation_key.lock().unwrap() = Some(CachedUserDelegationKey {
            key: key.clone(),
        });

        Ok(key)
    }

    async fn fetch_user_delegation_key(
        &self,
        signed_start: OffsetDateTime,
        signed_expiry: OffsetDateTime,
    ) -> Result<UserDelegationKey, AppError> {
        let token = blob_access_token().await?;
        let request_body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><KeyInfo><Start>{}</Start><Expiry>{}</Expiry></KeyInfo>"#,
            format_iso_8601(signed_start)?,
            format_iso_8601(signed_expiry)?,
        );

        let mut headers = blob_request_headers(Some("application/xml"))?;
        headers.insert(
            "x-ms-version",
            HeaderValue::from_static(STORAGE_SERVICE_VERSION),
        );

        let request_url = self
            .account_url
            .join(USER_DELEGATION_KEY_URL_SUFFIX)
            .map_err(|error| {
                AppError::config_with_source(
                    "Failed to build the user delegation key request URL.",
                    error,
                )
            })?;
        let response = self
            .client
            .post(request_url)
            .bearer_auth(token)
            .headers(headers)
            .body(request_body)
            .send()
            .await
            .map_err(|error| {
                AppError::internal_with_source(
                    "Failed to request an Azure Blob user delegation key.",
                    error,
                )
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(match status {
                StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN => AppError::request(body),
                _ => AppError::status(status, body),
            });
        }

        let xml = response.text().await.map_err(|error| {
            AppError::internal_with_source(
                "Failed to read the Azure Blob user delegation key response.".to_string(),
                error,
            )
        })?;
        let xml = xml.trim_start_matches('\u{feff}');
        xml_from_str::<UserDelegationKeyResponse>(xml)
            .map_err(|error| {
                AppError::internal_with_source(
                    "Failed to decode the Azure Blob user delegation key response.",
                    error,
                )
            })?
            .into_key()
    }

    async fn append_sas_query(
        &self,
        url: &mut Url,
        key_path: &str,
        key: &UserDelegationKey,
        permissions: &str,
        expires_at: OffsetDateTime,
        start_at: OffsetDateTime,
    ) -> Result<(), AppError> {
        let canonicalized_resource = self.canonicalized_resource(key_path)?;
        let signature = user_delegation_signature(
            key,
            permissions,
            &canonicalized_resource,
            start_at,
            expires_at,
        )?;

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("skoid", &key.signed_oid);
            query.append_pair("sktid", &key.signed_tid);
            query.append_pair("skt", &format_iso_8601(key.signed_start)?);
            query.append_pair("ske", &format_iso_8601(key.signed_expiry)?);
            query.append_pair("sks", &key.signed_service);
            query.append_pair("skv", &key.signed_version);
            query.append_pair("sv", SAS_VERSION);
            query.append_pair("sp", permissions);
            query.append_pair("sr", "b");
            query.append_pair("st", &format_iso_8601(start_at)?);
            query.append_pair("se", &format_iso_8601(expires_at)?);
            query.append_pair("spr", "https");
            query.append_pair("sig", &signature);
        }
        Ok(())
    }
}

#[async_trait]
impl ArtifactStore for BlobArtifactStore {
    async fn create_download_access(
        &self,
        path: &str,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        let normalized = normalize_artifact_path(path)?;
        let key = self.cached_user_delegation_key(expires_at).await?;
        let start_at = OffsetDateTime::now_utc() - TimeDuration::minutes(5);
        let mut url = self.blob_url(&normalized)?;
        self.append_sas_query(&mut url, &normalized, &key, "r", expires_at, start_at)
            .await?;

        Ok(ArtifactAccessGrant {
            expires_at: format_iso_8601(expires_at)?,
            headers: BTreeMap::new(),
            method: "GET",
            url: url.to_string(),
        })
    }

    async fn create_upload_access(
        &self,
        path: &str,
        content_type: Option<&str>,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        let normalized = normalize_artifact_path(path)?;
        let key = self.cached_user_delegation_key(expires_at).await?;
        let start_at = OffsetDateTime::now_utc() - TimeDuration::minutes(5);
        let mut url = self.blob_url(&normalized)?;
        self.append_sas_query(&mut url, &normalized, &key, "cw", expires_at, start_at)
            .await?;

        let mut headers = BTreeMap::new();
        headers.insert(
            "Content-Type".to_string(),
            artifact_content_type(&normalized, content_type),
        );
        headers.insert("x-ms-blob-type".to_string(), "BlockBlob".to_string());
        headers.insert(
            "x-ms-version".to_string(),
            STORAGE_SERVICE_VERSION.to_string(),
        );

        Ok(ArtifactAccessGrant {
            expires_at: format_iso_8601(expires_at)?,
            headers,
            method: "PUT",
            url: url.to_string(),
        })
    }

    async fn download_bytes(&self, path: &str) -> Result<(String, Vec<u8>), AppError> {
        let normalized = normalize_artifact_path(path)?;
        let response = self
            .request(
                Method::GET,
                self.blob_url(&normalized)?,
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

    async fn metadata(&self, path: &str) -> Result<Option<ArtifactMetadata>, AppError> {
        let normalized = normalize_artifact_path(path)?;
        let Some(response) = self
            .request_optional(
                Method::HEAD,
                self.blob_url(&normalized)?,
                blob_request_headers(None)?,
            )
            .await?
        else {
            return Ok(None);
        };

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let size = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);

        Ok(Some(ArtifactMetadata { content_type, size }))
    }

    async fn upload_bytes(
        &self,
        path: &str,
        content_type: Option<&str>,
        content: Vec<u8>,
    ) -> Result<ArtifactMetadata, AppError> {
        let normalized = normalize_artifact_path(path)?;
        let resolved_content_type = artifact_content_type(&normalized, content_type);
        let mut headers = blob_request_headers(Some(&resolved_content_type))?;
        headers.insert("x-ms-blob-type", HeaderValue::from_static("BlockBlob"));
        self.request(
            Method::PUT,
            self.blob_url(&normalized)?,
            headers,
            Some(content.clone()),
        )
        .await?;

        Ok(ArtifactMetadata {
            content_type: resolved_content_type,
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

fn storage_account_name(account_url: &Url) -> Result<String, AppError> {
    if let Some(host) = account_url.host_str()
        && host != "127.0.0.1"
        && host != "localhost"
    {
        return host
            .split('.')
            .next()
            .map(ToOwned::to_owned)
            .ok_or_else(|| AppError::config("Blob account URL is missing an account name."));
    }

    account_url
        .path_segments()
        .and_then(|mut segments| segments.find(|segment| !segment.is_empty()))
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::config("Blob account URL is missing an account name."))
}

fn user_delegation_signature(
    key: &UserDelegationKey,
    permissions: &str,
    canonicalized_resource: &str,
    start_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<String, AppError> {
    let string_to_sign =
        user_delegation_string_to_sign(key, permissions, canonicalized_resource, start_at, expires_at)?;

    let signing_key = BASE64_STANDARD.decode(key.value.as_bytes()).map_err(|error| {
        AppError::internal_with_source(
            "Failed to decode the Azure Blob user delegation key.",
            error,
        )
    })?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&signing_key).expect("hmac accepts arbitrary key lengths");
    mac.update(string_to_sign.as_bytes());
    Ok(BASE64_STANDARD.encode(mac.finalize().into_bytes()))
}

fn user_delegation_string_to_sign(
    key: &UserDelegationKey,
    permissions: &str,
    canonicalized_resource: &str,
    start_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<String, AppError> {
    Ok([
        permissions.to_string(),
        format_iso_8601(start_at)?,
        format_iso_8601(expires_at)?,
        canonicalized_resource.to_string(),
        key.signed_oid.clone(),
        key.signed_tid.clone(),
        format_iso_8601(key.signed_start)?,
        format_iso_8601(key.signed_expiry)?,
        key.signed_service.clone(),
        key.signed_version.clone(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        "https".to_string(),
        SAS_VERSION.to_string(),
        "b".to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    ]
    .join("\n"))
}

#[derive(Clone, Debug)]
struct UserDelegationKey {
    signed_oid: String,
    signed_tid: String,
    signed_start: OffsetDateTime,
    signed_expiry: OffsetDateTime,
    signed_service: String,
    signed_version: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserDelegationKeyResponse {
    signed_oid: String,
    signed_tid: String,
    signed_start: String,
    signed_expiry: String,
    signed_service: String,
    signed_version: String,
    value: String,
}

impl UserDelegationKeyResponse {
    fn into_key(self) -> Result<UserDelegationKey, AppError> {
        Ok(UserDelegationKey {
            signed_oid: self.signed_oid,
            signed_tid: self.signed_tid,
            signed_start: parse_user_delegation_time(
                &self.signed_start,
                "Failed to parse the Azure Blob user delegation start time.",
            )?,
            signed_expiry: parse_user_delegation_time(
                &self.signed_expiry,
                "Failed to parse the Azure Blob user delegation expiry time.",
            )?,
            signed_service: self.signed_service,
            signed_version: self.signed_version,
            value: self.value,
        })
    }
}

fn parse_user_delegation_time(value: &str, message: &str) -> Result<OffsetDateTime, AppError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| AppError::internal_with_source(message.to_string(), error))
}

#[cfg(test)]
mod tests {
    use reqwest::Method;
    use time::{Duration as TimeDuration, OffsetDateTime};

    use super::{
        format_iso_8601, normalize_artifact_path, output_path_for_run, scope_root,
        upload_path_for_file, user_delegation_string_to_sign, validate_input_path,
        verify_local_artifact_access, UserDelegationKey,
        UserDelegationKeyResponse,
    };
    use crate::session::Scope;

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
        assert!(validate_input_path(
            &scope,
            "workspaces/workspace-a/configs/config-v1/uploads/upl_123/input.xlsx"
        )
        .is_ok());
        assert!(validate_input_path(
            &scope,
            "workspaces/workspace-a/configs/config-v1/runs/run_123/output/normalized.xlsx"
        )
        .is_err());
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

        assert!(verify_local_artifact_access(
            "workspaces/a/configs/b/uploads/u/input.xlsx",
            &Method::PUT,
            &token,
            "secret",
            OffsetDateTime::now_utc(),
        )
        .is_ok());
        assert!(verify_local_artifact_access(
            "workspaces/a/configs/b/uploads/u/input.xlsx",
            &Method::GET,
            &token,
            "secret",
            OffsetDateTime::now_utc(),
        )
        .is_err());
    }

    #[test]
    fn parses_user_delegation_key_xml_with_utf8_bom() {
        let response = quick_xml::de::from_str::<UserDelegationKeyResponse>(
            "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?><UserDelegationKey><SignedOid>oid</SignedOid><SignedTid>tid</SignedTid><SignedStart>2026-03-29T22:30:00Z</SignedStart><SignedExpiry>2026-03-29T23:40:00Z</SignedExpiry><SignedService>b</SignedService><SignedVersion>2024-11-04</SignedVersion><Value>dmFsdWU=</Value></UserDelegationKey>",
        )
        .unwrap();

        assert_eq!(response.signed_oid, "oid");
        assert_eq!(response.signed_tid, "tid");
        assert_eq!(response.signed_version, "2024-11-04");
    }

    #[test]
    fn converts_user_delegation_key_times_with_z_suffix() {
        let key = UserDelegationKeyResponse {
            signed_oid: "oid".to_string(),
            signed_tid: "tid".to_string(),
            signed_start: "2026-03-29T22:30:00Z".to_string(),
            signed_expiry: "2026-03-29T23:40:00Z".to_string(),
            signed_service: "b".to_string(),
            signed_version: "2024-11-04".to_string(),
            value: "dmFsdWU=".to_string(),
        }
        .into_key()
        .unwrap();

        assert_eq!(format_iso_8601(key.signed_start).unwrap(), "2026-03-29T22:30:00Z");
        assert_eq!(format_iso_8601(key.signed_expiry).unwrap(), "2026-03-29T23:40:00Z");
    }

    #[test]
    fn user_delegation_string_to_sign_includes_all_blob_placeholders() {
        let key = UserDelegationKey {
            signed_oid: "oid".to_string(),
            signed_tid: "tid".to_string(),
            signed_start: OffsetDateTime::UNIX_EPOCH,
            signed_expiry: OffsetDateTime::UNIX_EPOCH + TimeDuration::hours(1),
            signed_service: "b".to_string(),
            signed_version: "2024-11-04".to_string(),
            value: "dmFsdWU=".to_string(),
        };
        let string_to_sign = user_delegation_string_to_sign(
            &key,
            "cw",
            "/blob/account/container/path",
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH + TimeDuration::minutes(15),
        )
        .unwrap();
        let fields = string_to_sign.split('\n').collect::<Vec<_>>();

        assert_eq!(fields.len(), 24);
        assert_eq!(fields[16], "b");
        assert_eq!(fields[17], "");
        assert_eq!(fields[18], "");
        assert_eq!(fields[19], "");
        assert_eq!(fields[20], "");
        assert_eq!(fields[21], "");
        assert_eq!(fields[22], "");
        assert_eq!(fields[23], "");
    }
}
