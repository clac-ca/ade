use std::time::Duration;

use reqwest::{
    Client, Method, Url,
    multipart::{Form, Part},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use utoipa::ToSchema;

use crate::{
    azure_auth::{access_token_with_developer_fallback, read_azure_client_id},
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    runs::RunTimings,
};

pub(crate) const AZURE_SHELL_API_VERSION: &str = "2025-10-02-preview";
const DEFAULT_AZURE_SESSION_AUDIENCE: &str = "https://dynamicsessions.io";
const SESSION_POOL_BEARER_TOKEN_ENV_NAME: &str = "ADE_SESSION_POOL_BEARER_TOKEN";
const SESSION_POOL_MANAGEMENT_ENDPOINT_ENV_NAME: &str = "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT";
const SESSION_POOL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_POOL_DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const SESSION_POOL_EXECUTION_TIMEOUT_BUFFER_SECONDS: u64 = 30;
const SESSION_POOL_UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionFile {
    pub filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified_time: Option<String>,
    pub size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionExecution {
    pub(crate) duration_ms: u64,
    pub(crate) status: String,
    pub(crate) stderr: String,
    pub(crate) stdout: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionOperationMetadata {
    pub(crate) operation_id: Option<String>,
    pub(crate) session_guid: Option<String>,
    pub(crate) timings: Option<RunTimings>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionOperationResult<T> {
    pub(crate) metadata: SessionOperationMetadata,
    pub(crate) value: T,
}

#[derive(Clone)]
pub(crate) struct SessionPoolClient {
    client: Client,
    managed_identity_client_id: Option<String>,
    pool_management_endpoint: Url,
    bearer_token_override: Option<String>,
}

impl SessionPoolClient {
    pub(crate) fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let pool_management_endpoint = read_optional_trimmed_string(
            env,
            SESSION_POOL_MANAGEMENT_ENDPOINT_ENV_NAME,
        )
        .ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {SESSION_POOL_MANAGEMENT_ENDPOINT_ENV_NAME}"
            ))
        })?;

        let mut endpoint = Url::parse(&format!(
            "{}/",
            pool_management_endpoint.trim_end_matches('/')
        ))
        .map_err(|error| {
            AppError::config_with_source(
                "Session pool endpoint is not a valid URL.".to_string(),
                error,
            )
        })?;
        endpoint.path_segments_mut().map_err(|()| {
            AppError::config("Session pool endpoint cannot be used as a base URL.".to_string())
        })?;

        Ok(Self {
            client: Client::builder()
                .connect_timeout(SESSION_POOL_CONNECT_TIMEOUT)
                .build()
                .map_err(|error| {
                    AppError::startup_with_source(
                        "Failed to build the session-pool HTTP client.",
                        error,
                    )
                })?,
            managed_identity_client_id: read_azure_client_id(env),
            pool_management_endpoint: endpoint,
            bearer_token_override: read_optional_trimmed_string(
                env,
                SESSION_POOL_BEARER_TOKEN_ENV_NAME,
            ),
        })
    }

    pub(crate) async fn execute(
        &self,
        identifier: &str,
        shell_command: String,
        timeout_in_seconds: Option<u64>,
    ) -> Result<SessionOperationResult<SessionExecution>, AppError> {
        let request = self
            .data_plane_request(Method::POST, &["executions"], identifier, &[])
            .await?
            .timeout(session_pool_execution_timeout(timeout_in_seconds))
            .json(&ShellExecutionRequest {
                shell_command,
                timeout_in_seconds,
            });
        let envelope: SessionOperationResult<ExecutionEnvelope> =
            parse_json_response(request, "call the session pool API").await?;

        Ok(SessionOperationResult {
            metadata: envelope.metadata,
            value: SessionExecution {
                duration_ms: envelope
                    .value
                    .result
                    .execution_time_in_milliseconds
                    .unwrap_or(0),
                status: envelope.value.status,
                stderr: envelope.value.result.stderr,
                stdout: envelope.value.result.stdout,
            },
        })
    }

    pub(crate) async fn upload_file(
        &self,
        identifier: &str,
        path: Option<&str>,
        filename: &str,
        content_type: Option<&str>,
        content: Vec<u8>,
    ) -> Result<SessionOperationResult<SessionFile>, AppError> {
        let mut part = Part::bytes(content).file_name(filename.to_string());
        if let Some(content_type) = content_type {
            part = part.mime_str(content_type).map_err(|error| {
                AppError::request(format!("Invalid uploaded file content type: {error}"))
            })?;
        }
        let query_pairs = path.map(|value| [("path", value)]);

        let request = self
            .data_plane_request(
                Method::POST,
                &["files"],
                identifier,
                query_pairs.as_ref().map_or(&[], |pairs| pairs.as_slice()),
            )
            .await?
            .timeout(SESSION_POOL_UPLOAD_TIMEOUT)
            .multipart(Form::new().part("file", part));
        let record: SessionOperationResult<AzureFileRecord> =
            parse_json_response(request, "upload a session pool file").await?;
        Ok(SessionOperationResult {
            metadata: record.metadata,
            value: SessionFile {
                filename: match record.value.directory.as_deref() {
                    Some("" | ".") | None => record.value.name,
                    Some(directory) => format!("{directory}/{}", record.value.name),
                },
                last_modified_time: record.value.last_modified_at,
                size: record.value.size_in_bytes,
            },
        })
    }

    pub(crate) async fn download_file(
        &self,
        identifier: &str,
        path: Option<&str>,
        filename: &str,
    ) -> Result<SessionOperationResult<Vec<u8>>, AppError> {
        let query_pairs = path.map(|value| [("path", value)]);
        let request = self
            .data_plane_request(
                Method::GET,
                &["files", filename, "content"],
                identifier,
                query_pairs.as_ref().map_or(&[], |pairs| pairs.as_slice()),
            )
            .await?
            .timeout(SESSION_POOL_UPLOAD_TIMEOUT);

        parse_bytes_response(request, "download a session pool file").await
    }

    async fn data_plane_request(
        &self,
        method: Method,
        path_segments: &[&str],
        identifier: &str,
        query_pairs: &[(&str, &str)],
    ) -> Result<reqwest::RequestBuilder, AppError> {
        let url = session_pool_url(
            &self.pool_management_endpoint,
            path_segments,
            identifier,
            query_pairs,
        );
        let request = self.client.request(method, url);
        let bearer_token = match &self.bearer_token_override {
            Some(token) => token.clone(),
            None => data_plane_token(self.managed_identity_client_id.clone()).await?,
        };

        Ok(request.bearer_auth(bearer_token))
    }
}

fn session_pool_execution_timeout(timeout_in_seconds: Option<u64>) -> Duration {
    match timeout_in_seconds {
        Some(timeout) => Duration::from_secs(
            timeout.saturating_add(SESSION_POOL_EXECUTION_TIMEOUT_BUFFER_SECONDS),
        ),
        None => SESSION_POOL_DEFAULT_REQUEST_TIMEOUT,
    }
}

#[derive(Deserialize)]
struct ExecutionEnvelope {
    status: String,
    result: ExecutionResultEnvelope,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionResultEnvelope {
    #[serde(default)]
    execution_time_in_milliseconds: Option<u64>,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    stdout: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzureFileRecord {
    directory: Option<String>,
    name: String,
    #[serde(default)]
    last_modified_at: Option<String>,
    size_in_bytes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShellExecutionRequest {
    shell_command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_in_seconds: Option<u64>,
}

async fn data_plane_token(client_id: Option<String>) -> Result<String, AppError> {
    let scope = format!("{DEFAULT_AZURE_SESSION_AUDIENCE}/.default");

    access_token_with_developer_fallback(
        scope.as_str(),
        client_id,
        "Failed to create the Azure developer credential.",
        "Failed to acquire an Azure access token for session pool calls.",
    )
    .await
}

fn session_pool_url(
    base_endpoint: &Url,
    path_segments: &[&str],
    identifier: &str,
    query_pairs: &[(&str, &str)],
) -> Url {
    let mut url = base_endpoint.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .expect("session pool endpoint was validated at startup");
        segments.pop_if_empty();
        for segment in path_segments {
            segments.push(segment);
        }
    }
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("identifier", identifier);
        query.append_pair("api-version", AZURE_SHELL_API_VERSION);
        for (name, value) in query_pairs {
            query.append_pair(name, value);
        }
    }
    url
}

async fn parse_json_response<T>(
    builder: reqwest::RequestBuilder,
    operation: &str,
) -> Result<SessionOperationResult<T>, AppError>
where
    T: DeserializeOwned,
{
    let response = builder
        .send()
        .await
        .map_err(|error| map_session_pool_transport_error(operation, error))?;
    let response = ensure_success_response(response).await?;
    let metadata = parse_response_metadata(response.headers());
    let value = response.json::<T>().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to decode the session-pool response while trying to {operation}."),
            error,
        )
    })?;

    Ok(SessionOperationResult { metadata, value })
}

async fn parse_bytes_response(
    builder: reqwest::RequestBuilder,
    operation: &str,
) -> Result<SessionOperationResult<Vec<u8>>, AppError> {
    let response = builder
        .send()
        .await
        .map_err(|error| map_session_pool_transport_error(operation, error))?;
    let response = ensure_success_response(response).await?;
    let metadata = parse_response_metadata(response.headers());
    let value = response.bytes().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to read the session-pool response while trying to {operation}."),
            error,
        )
    })?;

    Ok(SessionOperationResult {
        metadata,
        value: value.to_vec(),
    })
}

async fn ensure_success_response(
    response: reqwest::Response,
) -> Result<reqwest::Response, AppError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    #[derive(Deserialize)]
    struct ErrorBody {
        error: Option<ErrorDetail>,
        message: Option<String>,
        title: Option<String>,
        errors: Option<std::collections::BTreeMap<String, Vec<String>>>,
    }

    #[derive(Deserialize)]
    struct ErrorDetail {
        message: Option<String>,
    }

    let bytes = response.bytes().await.map_err(|error| {
        AppError::internal_with_source(
            "Failed to read the session-pool error response.".to_string(),
            error,
        )
    })?;
    let message = if let Ok(body) = serde_json::from_slice::<ErrorBody>(&bytes) {
        if let Some(message) = body
            .error
            .and_then(|error| error.message)
            .or(body.message)
            .or(body.title)
        {
            message
        } else if let Some(message) = body
            .errors
            .and_then(|errors| errors.into_values().flatten().next())
        {
            message
        } else {
            "The session pool did not return an error message.".to_string()
        }
    } else {
        let fallback = String::from_utf8_lossy(&bytes).trim().to_string();
        if fallback.is_empty() {
            "The session pool did not return an error message.".to_string()
        } else {
            fallback
        }
    };

    Err(map_session_pool_http_error(status, message))
}

fn parse_response_metadata(headers: &reqwest::header::HeaderMap) -> SessionOperationMetadata {
    let header_string = |name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    let header_u64 = |name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
    };
    let timings = RunTimings {
        allocation_time_ms: header_u64("x-ms-allocation-time"),
        container_execution_duration_ms: header_u64("x-ms-container-execution-duration"),
        overall_execution_time_ms: header_u64("x-ms-overall-execution-time"),
        preparation_time_ms: header_u64("x-ms-preparation-time"),
    };

    SessionOperationMetadata {
        operation_id: header_string("operation-id"),
        session_guid: header_string("x-ms-session-guid"),
        timings: if timings.allocation_time_ms.is_some()
            || timings.container_execution_duration_ms.is_some()
            || timings.overall_execution_time_ms.is_some()
            || timings.preparation_time_ms.is_some()
        {
            Some(timings)
        } else {
            None
        },
    }
}

fn map_session_pool_transport_error(operation: &str, error: reqwest::Error) -> AppError {
    if error.is_timeout() {
        return AppError::unavailable(format!(
            "The session pool request timed out while trying to {operation}."
        ));
    }

    if error.is_connect() || error.is_request() {
        return AppError::unavailable(format!(
            "The session pool is unavailable while trying to {operation}."
        ));
    }

    AppError::internal_with_source(format!("Failed to {operation}."), error)
}

fn map_session_pool_http_error(status: reqwest::StatusCode, message: String) -> AppError {
    match status {
        reqwest::StatusCode::NOT_FOUND => AppError::not_found(message),
        reqwest::StatusCode::BAD_REQUEST
        | reqwest::StatusCode::UNPROCESSABLE_ENTITY
        | reqwest::StatusCode::CONFLICT => AppError::request(message),
        reqwest::StatusCode::SERVICE_UNAVAILABLE => AppError::unavailable(message),
        _ => AppError::status(status, message),
    }
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;
    use reqwest::{StatusCode as ReqwestStatusCode, Url};

    use crate::config::EnvBag;

    use super::{
        SESSION_POOL_BEARER_TOKEN_ENV_NAME, SessionPoolClient, map_session_pool_http_error,
        session_pool_url,
    };

    #[test]
    fn session_pool_http_errors_preserve_upstream_status_codes() {
        let response = map_session_pool_http_error(
            ReqwestStatusCode::TOO_MANY_REQUESTS,
            "Too many requests".to_string(),
        )
        .into_response();

        assert_eq!(response.status().as_u16(), 429);
    }

    #[test]
    fn file_download_urls_encode_path_segments_without_losing_slashes() {
        let url = session_pool_url(
            &Url::parse("https://example.com/session-pool/").unwrap(),
            &["files", "input #1.xlsx", "content"],
            "cfg-123",
            &[("path", "runs/run 1/output")],
        );

        assert_eq!(
            url.as_str(),
            "https://example.com/session-pool/files/input%20%231.xlsx/content?identifier=cfg-123&api-version=2025-10-02-preview&path=runs%2Frun+1%2Foutput"
        );
    }

    #[test]
    fn list_file_urls_append_recursive_query_parameters() {
        let url = session_pool_url(
            &Url::parse("https://example.com/session-pool/").unwrap(),
            &["files"],
            "cfg-123",
            &[("recursive", "true")],
        );

        assert_eq!(
            url.as_str(),
            "https://example.com/session-pool/files?identifier=cfg-123&api-version=2025-10-02-preview&recursive=true"
        );
    }

    fn env(entries: &[(&str, &str)]) -> EnvBag {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn explicit_bearer_token_override_is_used_without_host_inspection() {
        let client = SessionPoolClient::from_env(&env(&[
            (
                "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT",
                "https://example.dynamicsessions.io",
            ),
            (SESSION_POOL_BEARER_TOKEN_ENV_NAME, "local-token"),
        ]))
        .unwrap();

        assert_eq!(client.bearer_token_override.as_deref(), Some("local-token"));
    }

    #[test]
    fn session_pool_client_defaults_to_azure_credentials_when_no_override_is_set() {
        let client = SessionPoolClient::from_env(&env(&[(
            "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT",
            "http://127.0.0.1:8014",
        )]))
        .unwrap();

        assert_eq!(client.bearer_token_override, None);
        assert_eq!(client.managed_identity_client_id, None);
    }

    #[test]
    fn session_pool_client_uses_the_runtime_azure_client_id_when_present() {
        let client = SessionPoolClient::from_env(&env(&[
            (
                "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT",
                "https://example.dynamicsessions.io",
            ),
            ("AZURE_CLIENT_ID", "runtime-client-id"),
        ]))
        .unwrap();

        assert_eq!(
            client.managed_identity_client_id.as_deref(),
            Some("runtime-client-id")
        );
    }
}
