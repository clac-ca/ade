use std::{collections::BTreeMap, sync::Mutex};

use async_trait::async_trait;
use azure_core::credentials::TokenCredential;
use azure_identity::{DeveloperToolsCredential, ManagedIdentityCredential};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use hmac::{Hmac, Mac};
use quick_xml::de::from_str as xml_from_str;
use reqwest::{
    Client, Method, StatusCode, Url,
    header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use sha2::Sha256;
use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::error::AppError;

use super::{
    ArtifactAccessGrant, ArtifactMetadata, ArtifactStore, BLOB_ACCOUNT_KEY_ENV_NAME,
    BLOB_ACCOUNT_URL_ENV_NAME, BLOB_PUBLIC_ACCOUNT_URL_ENV_NAME, BLOB_RUNTIME_ACCOUNT_URL_ENV_NAME,
    artifact_content_type, format_iso_8601, normalize_artifact_path,
};

const BLOB_CORS_MAX_AGE_SECONDS: u64 = 3600;
const SAS_VERSION: &str = "2024-11-04";
const STORAGE_AUDIENCE_SCOPE: &str = "https://storage.azure.com/.default";
const STORAGE_SERVICE_VERSION: &str = "2024-11-04";
const USER_DELEGATION_KEY_REFRESH_BUFFER_MINUTES: i64 = 5;
const USER_DELEGATION_KEY_TTL_MINUTES: i64 = 60;
const USER_DELEGATION_KEY_URL_SUFFIX: &str = "/?restype=service&comp=userdelegationkey";

#[derive(Clone)]
struct CachedUserDelegationKey {
    key: UserDelegationKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlobAuthMode {
    SharedKey,
    UserDelegation,
}

pub(super) struct BlobArtifactStore {
    account_name: String,
    account_url: Url,
    auth_mode: BlobAuthMode,
    client: Client,
    container: String,
    local_blob_account_key: Option<String>,
    local_cors_allowed_origins: Vec<String>,
    local_setup_complete: Mutex<bool>,
    public_account_url: Url,
    runtime_account_url: Url,
    user_delegation_key: Mutex<Option<CachedUserDelegationKey>>,
}

impl BlobArtifactStore {
    pub(super) fn new(
        account_url: String,
        public_account_url: Option<String>,
        runtime_account_url: Option<String>,
        container: String,
        local_blob_account_key: Option<String>,
        local_cors_allowed_origins: Vec<String>,
    ) -> Result<Self, AppError> {
        let account_url = Url::parse(&account_url).map_err(|error| {
            AppError::config_with_source(
                format!("{BLOB_ACCOUNT_URL_ENV_NAME} is not a valid URL."),
                error,
            )
        })?;
        let account_name = storage_account_name(&account_url)?;
        let auth_mode = if local_blob_account_key.is_some() {
            BlobAuthMode::SharedKey
        } else {
            BlobAuthMode::UserDelegation
        };
        let public_account_url =
            parse_optional_blob_base_url(BLOB_PUBLIC_ACCOUNT_URL_ENV_NAME, public_account_url)?
                .unwrap_or_else(|| account_url.clone());
        let runtime_account_url =
            parse_optional_blob_base_url(BLOB_RUNTIME_ACCOUNT_URL_ENV_NAME, runtime_account_url)?
                .unwrap_or_else(|| account_url.clone());

        Ok(Self {
            account_name,
            account_url,
            auth_mode,
            client: Client::new(),
            container,
            local_blob_account_key,
            local_cors_allowed_origins,
            local_setup_complete: Mutex::new(false),
            public_account_url,
            runtime_account_url,
            user_delegation_key: Mutex::new(None),
        })
    }

    fn blob_url_from_base(&self, base_url: &Url, key: &str) -> Result<Url, AppError> {
        let mut url = base_url.clone();
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

    fn blob_url(&self, key: &str) -> Result<Url, AppError> {
        self.blob_url_from_base(&self.account_url, key)
    }

    fn canonicalized_resource(&self, key: &str) -> Result<String, AppError> {
        let normalized = normalize_artifact_path(key)?;
        Ok(format!(
            "/blob/{}/{}/{}",
            self.account_name, self.container, normalized
        ))
    }

    fn shared_key(&self) -> Result<&str, AppError> {
        self.local_blob_account_key
            .as_deref()
            .ok_or_else(|| AppError::internal("Local Blob account key is not configured."))
    }

    async fn ensure_local_blob_ready(&self) -> Result<(), AppError> {
        if self.auth_mode != BlobAuthMode::SharedKey {
            return Ok(());
        }

        if *self.local_setup_complete.lock().unwrap() {
            return Ok(());
        }

        if !self.local_cors_allowed_origins.is_empty() {
            self.configure_local_cors().await?;
        }
        self.ensure_container_exists().await?;
        *self.local_setup_complete.lock().unwrap() = true;
        Ok(())
    }

    async fn configure_local_cors(&self) -> Result<(), AppError> {
        let allowed_origins = self.local_cors_allowed_origins.join(",");
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><StorageServiceProperties><Cors><CorsRule><AllowedOrigins>{allowed_origins}</AllowedOrigins><AllowedMethods>GET,HEAD,OPTIONS,PUT</AllowedMethods><MaxAgeInSeconds>{BLOB_CORS_MAX_AGE_SECONDS}</MaxAgeInSeconds><ExposedHeaders>etag,x-ms-*</ExposedHeaders><AllowedHeaders>content-type,x-ms-*</AllowedHeaders></CorsRule></Cors><DefaultServiceVersion>{STORAGE_SERVICE_VERSION}</DefaultServiceVersion></StorageServiceProperties>"
        );
        let mut headers = blob_request_headers(Some("application/xml"))?;
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&body.len().to_string()).map_err(|error| {
                AppError::internal_with_source(
                    "Failed to encode the local Blob service properties request length."
                        .to_string(),
                    error,
                )
            })?,
        );
        let mut request_url = self.account_url.clone();
        request_url
            .query_pairs_mut()
            .append_pair("restype", "service")
            .append_pair("comp", "properties");
        self.request(Method::PUT, request_url, headers, Some(body.into_bytes()))
            .await?;
        Ok(())
    }

    async fn ensure_container_exists(&self) -> Result<(), AppError> {
        let mut url = self.account_url.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|()| {
                AppError::config("Blob account URL cannot be used as a base URL.".to_string())
            })?;
            segments.pop_if_empty();
            segments.push(&self.container);
        }
        url.query_pairs_mut().append_pair("restype", "container");
        let headers = blob_request_headers(None)?;
        match self.request(Method::PUT, url, headers, None).await {
            Ok(_) => Ok(()),
            Err(AppError::Response {
                status: StatusCode::CONFLICT,
                ..
            }) => Ok(()),
            Err(AppError::Request(message))
                if message.contains("ContainerAlreadyExists")
                    || message.contains("container already exists") =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn create_access_grant(
        &self,
        base_url: &Url,
        key_path: &str,
        permissions: &str,
        method: &'static str,
        content_type: Option<&str>,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        let normalized = normalize_artifact_path(key_path)?;
        let mut signed_url = self.blob_url_from_base(base_url, &normalized)?;
        let start_at = OffsetDateTime::now_utc() - TimeDuration::minutes(5);

        match self.auth_mode {
            BlobAuthMode::UserDelegation => {
                let key = self.cached_user_delegation_key(expires_at).await?;
                self.append_user_delegation_sas_query(
                    &mut signed_url,
                    &normalized,
                    &key,
                    permissions,
                    expires_at,
                    start_at,
                )
                .await?;
            }
            BlobAuthMode::SharedKey => {
                self.append_shared_key_sas_query(
                    &mut signed_url,
                    &normalized,
                    permissions,
                    expires_at,
                    start_at,
                )?;
            }
        }

        Ok(ArtifactAccessGrant {
            expires_at: format_iso_8601(expires_at)?,
            headers: access_headers(&normalized, method, content_type),
            method,
            url: signed_url.to_string(),
        })
    }

    fn shared_key_authorization(
        &self,
        method: &Method,
        url: &Url,
        headers: &HeaderMap,
        body_len: Option<usize>,
    ) -> Result<String, AppError> {
        let string_to_sign =
            shared_key_string_to_sign(&self.account_name, method, url, headers, body_len)?;
        let signing_key = BASE64_STANDARD
            .decode(self.shared_key()?.as_bytes())
            .map_err(|error| {
                AppError::config_with_source(
                    format!("{BLOB_ACCOUNT_KEY_ENV_NAME} is not valid base64."),
                    error,
                )
            })?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&signing_key)
            .expect("hmac accepts arbitrary key lengths");
        mac.update(string_to_sign.as_bytes());
        Ok(format!(
            "SharedKey {}:{}",
            self.account_name,
            BASE64_STANDARD.encode(mac.finalize().into_bytes())
        ))
    }

    async fn send_authenticated_request(
        &self,
        method: Method,
        url: Url,
        mut headers: HeaderMap,
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, AppError> {
        let mut builder = self.client.request(method.clone(), url.clone());

        match self.auth_mode {
            BlobAuthMode::UserDelegation => {
                builder = builder.bearer_auth(blob_access_token().await?);
            }
            BlobAuthMode::SharedKey => {
                let body_len = body.as_ref().map(Vec::len);
                let authorization =
                    self.shared_key_authorization(&method, &url, &headers, body_len)?;
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&authorization).map_err(|error| {
                        AppError::internal_with_source(
                            "Failed to encode the local Blob authorization header.".to_string(),
                            error,
                        )
                    })?,
                );
            }
        }

        builder = builder.headers(headers);
        if let Some(body) = body {
            builder = builder.body(body);
        }

        builder.send().await.map_err(|error| {
            AppError::internal_with_source("Failed to call Azure Blob Storage.".to_string(), error)
        })
    }

    async fn request(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, AppError> {
        let response = self
            .send_authenticated_request(method, url, headers, body)
            .await?;

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
        let response = self
            .send_authenticated_request(method, url, headers, None)
            .await?;

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
        if self.auth_mode != BlobAuthMode::UserDelegation {
            return Err(AppError::internal(
                "User delegation SAS is only available for Azure Blob Storage.",
            ));
        }

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

        *self.user_delegation_key.lock().unwrap() =
            Some(CachedUserDelegationKey { key: key.clone() });

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

    async fn append_user_delegation_sas_query(
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

    fn append_shared_key_sas_query(
        &self,
        url: &mut Url,
        key_path: &str,
        permissions: &str,
        expires_at: OffsetDateTime,
        start_at: OffsetDateTime,
    ) -> Result<(), AppError> {
        let canonicalized_resource = self.canonicalized_resource(key_path)?;
        let signature = shared_key_sas_signature(
            self.shared_key()?,
            permissions,
            &canonicalized_resource,
            start_at,
            expires_at,
        )?;

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("sv", SAS_VERSION);
            query.append_pair("sp", permissions);
            query.append_pair("sr", "b");
            query.append_pair("st", &format_iso_8601(start_at)?);
            query.append_pair("se", &format_iso_8601(expires_at)?);
            query.append_pair("spr", "https,http");
            query.append_pair("sig", &signature);
        }
        Ok(())
    }
}

#[async_trait]
impl ArtifactStore for BlobArtifactStore {
    async fn create_browser_upload_access(
        &self,
        path: &str,
        content_type: Option<&str>,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        self.ensure_local_blob_ready().await?;
        self.create_access_grant(
            &self.public_account_url,
            path,
            "cw",
            "PUT",
            content_type,
            expires_at,
        )
        .await
    }

    async fn create_download_access(
        &self,
        path: &str,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        self.ensure_local_blob_ready().await?;
        self.create_access_grant(
            &self.runtime_account_url,
            path,
            "r",
            "GET",
            None,
            expires_at,
        )
        .await
    }

    async fn create_upload_access(
        &self,
        path: &str,
        content_type: Option<&str>,
        expires_at: OffsetDateTime,
    ) -> Result<ArtifactAccessGrant, AppError> {
        self.ensure_local_blob_ready().await?;
        self.create_access_grant(
            &self.runtime_account_url,
            path,
            "cw",
            "PUT",
            content_type,
            expires_at,
        )
        .await
    }

    async fn download_bytes(&self, path: &str) -> Result<(String, Vec<u8>), AppError> {
        self.ensure_local_blob_ready().await?;
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
        self.ensure_local_blob_ready().await?;
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
        self.ensure_local_blob_ready().await?;
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

fn access_headers(
    path: &str,
    method: &'static str,
    content_type: Option<&str>,
) -> BTreeMap<String, String> {
    if method != "PUT" {
        return BTreeMap::new();
    }

    BTreeMap::from([
        (
            "Content-Type".to_string(),
            artifact_content_type(path, content_type),
        ),
        ("x-ms-blob-type".to_string(), "BlockBlob".to_string()),
        (
            "x-ms-version".to_string(),
            STORAGE_SERVICE_VERSION.to_string(),
        ),
    ])
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
        AppError::internal_with_source("Failed to initialize Azure developer credentials.", error)
    })?;
    credential
        .get_token(&[STORAGE_AUDIENCE_SCOPE], None)
        .await
        .map(|token| token.token.secret().to_string())
        .map_err(|error| {
            AppError::internal_with_source("Failed to get an Azure Blob access token.", error)
        })
}

fn storage_account_name(account_url: &Url) -> Result<String, AppError> {
    let host = account_url.host_str().ok_or_else(|| {
        AppError::config(format!("{BLOB_ACCOUNT_URL_ENV_NAME} must include a host."))
    })?;
    if host == "127.0.0.1" || host == "localhost" || host == "host.docker.internal" {
        let account_name = account_url
            .path_segments()
            .and_then(|mut segments| segments.find(|segment| !segment.is_empty()))
            .ok_or_else(|| {
                AppError::config(format!(
                    "{BLOB_ACCOUNT_URL_ENV_NAME} must include the storage account name in the path when using Azurite."
                ))
            })?;
        return Ok(account_name.to_string());
    }

    Ok(host.split('.').next().unwrap_or_default().to_string())
}

fn parse_optional_blob_base_url(
    env_name: &str,
    value: Option<String>,
) -> Result<Option<Url>, AppError> {
    value
        .map(|value| {
            Url::parse(&value).map_err(|error| {
                AppError::config_with_source(format!("{env_name} is not a valid URL."), error)
            })
        })
        .transpose()
}

fn canonicalized_headers(headers: &HeaderMap) -> Result<String, AppError> {
    let mut canonicalized = headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();
            if !name.starts_with("x-ms-") {
                return None;
            }

            Some(
                value
                    .to_str()
                    .map(|value| (name, value.split_whitespace().collect::<Vec<_>>().join(" "))),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            AppError::internal_with_source(
                "Failed to read Azure Blob request headers.".to_string(),
                error,
            )
        })?;
    canonicalized.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(canonicalized
        .into_iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect())
}

fn header_value(headers: &HeaderMap, name: reqwest::header::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn canonicalized_resource_for_request(account_name: &str, url: &Url) -> String {
    let mut canonicalized = format!("/{account_name}{}", url.path());
    let mut query = url
        .query_pairs()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.into_owned()))
        .collect::<Vec<_>>();
    query.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, value) in query {
        canonicalized.push('\n');
        canonicalized.push_str(&name);
        canonicalized.push(':');
        canonicalized.push_str(&value);
    }
    canonicalized
}

fn shared_key_string_to_sign(
    account_name: &str,
    method: &Method,
    url: &Url,
    headers: &HeaderMap,
    body_len: Option<usize>,
) -> Result<String, AppError> {
    let content_length = match body_len {
        Some(0) | None => String::new(),
        Some(value) => value.to_string(),
    };
    Ok([
        method.as_str().to_string(),
        String::new(),
        String::new(),
        content_length,
        String::new(),
        header_value(headers, CONTENT_TYPE),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        format!(
            "{}{}",
            canonicalized_headers(headers)?,
            canonicalized_resource_for_request(account_name, url)
        ),
    ]
    .join("\n"))
}

fn shared_key_sas_signature(
    account_key: &str,
    permissions: &str,
    canonicalized_resource: &str,
    start_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<String, AppError> {
    let string_to_sign =
        shared_key_sas_string_to_sign(permissions, canonicalized_resource, start_at, expires_at)?;
    let signing_key = BASE64_STANDARD
        .decode(account_key.as_bytes())
        .map_err(|error| {
            AppError::config_with_source(
                format!("{BLOB_ACCOUNT_KEY_ENV_NAME} is not valid base64."),
                error,
            )
        })?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&signing_key).expect("hmac accepts arbitrary key lengths");
    mac.update(string_to_sign.as_bytes());
    Ok(BASE64_STANDARD.encode(mac.finalize().into_bytes()))
}

fn shared_key_sas_string_to_sign(
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
        String::new(),
        String::new(),
        "https,http".to_string(),
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

fn user_delegation_signature(
    key: &UserDelegationKey,
    permissions: &str,
    canonicalized_resource: &str,
    start_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<String, AppError> {
    let string_to_sign = user_delegation_string_to_sign(
        key,
        permissions,
        canonicalized_resource,
        start_at,
        expires_at,
    )?;
    let signing_key = BASE64_STANDARD
        .decode(key.value.as_bytes())
        .map_err(|error| {
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

#[derive(Clone, Debug, Deserialize)]
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
    use reqwest::{
        Method, Url,
        header::{CONTENT_TYPE, HeaderMap, HeaderValue},
    };
    use time::{Duration as TimeDuration, OffsetDateTime};

    use super::{
        UserDelegationKey, UserDelegationKeyResponse, canonicalized_resource_for_request,
        format_iso_8601, shared_key_sas_string_to_sign, shared_key_string_to_sign,
        user_delegation_string_to_sign,
    };

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

        assert_eq!(
            format_iso_8601(key.signed_start).unwrap(),
            "2026-03-29T22:30:00Z"
        );
        assert_eq!(
            format_iso_8601(key.signed_expiry).unwrap(),
            "2026-03-29T23:40:00Z"
        );
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

    #[test]
    fn shared_key_sas_string_to_sign_includes_all_blob_placeholders() {
        let string_to_sign = shared_key_sas_string_to_sign(
            "cw",
            "/blob/account/container/path",
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH + TimeDuration::minutes(15),
        )
        .unwrap();
        let fields = string_to_sign.split('\n').collect::<Vec<_>>();

        assert_eq!(fields.len(), 16);
        assert_eq!(fields[8], "b");
        assert_eq!(fields[9], "");
        assert_eq!(fields[10], "");
        assert_eq!(fields[11], "");
        assert_eq!(fields[12], "");
        assert_eq!(fields[13], "");
        assert_eq!(fields[14], "");
        assert_eq!(fields[15], "");
    }

    #[test]
    fn local_blob_requests_keep_the_account_name_in_the_signed_path() {
        let url = Url::parse("http://127.0.0.1:10000/devstoreaccount1/documents?restype=container")
            .unwrap();

        assert_eq!(
            canonicalized_resource_for_request("devstoreaccount1", &url),
            "/devstoreaccount1/devstoreaccount1/documents\nrestype:container"
        );
    }

    #[test]
    fn shared_key_request_string_to_sign_matches_blob_shape() {
        let url =
            Url::parse("http://127.0.0.1:10000/devstoreaccount1?comp=properties&restype=service")
                .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert(
            "x-ms-date",
            HeaderValue::from_static("Sun, 29 Mar 2026 23:55:00 GMT"),
        );
        headers.insert("x-ms-version", HeaderValue::from_static("2024-11-04"));
        let string_to_sign =
            shared_key_string_to_sign("devstoreaccount1", &Method::PUT, &url, &headers, None)
                .unwrap();

        assert_eq!(
            string_to_sign,
            concat!(
                "PUT\n",
                "\n",
                "\n",
                "\n",
                "\n",
                "application/octet-stream\n",
                "\n",
                "\n",
                "\n",
                "\n",
                "\n",
                "\n",
                "x-ms-date:Sun, 29 Mar 2026 23:55:00 GMT\n",
                "x-ms-version:2024-11-04\n",
                "/devstoreaccount1/devstoreaccount1\n",
                "comp:properties\n",
                "restype:service"
            )
        );
    }
}
