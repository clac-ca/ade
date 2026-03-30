use azure_core::credentials::TokenCredential;
use azure_identity::{DeveloperToolsCredential, ManagedIdentityCredential};
use reqwest::{
    Client, Method, Url,
    multipart::{Form, Part},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use utoipa::ToSchema;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    runs::RunTimings,
};

const DEFAULT_AZURE_SESSION_API_VERSION: &str = "2025-10-02-preview";
const DEFAULT_AZURE_SESSION_AUDIENCE: &str = "https://dynamicsessions.io";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionFile {
    pub filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified_time: Option<String>,
    pub size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PythonExecution {
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

pub(crate) struct SessionPoolClient {
    client: Client,
    pool_management_endpoint: Url,
    uses_azure_auth: bool,
}

impl SessionPoolClient {
    pub(crate) fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let pool_management_endpoint =
            read_optional_trimmed_string(env, "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT").ok_or_else(
                || {
                    AppError::config(
                "Missing required environment variable: ADE_SESSION_POOL_MANAGEMENT_ENDPOINT"
                    .to_string(),
            )
                },
            )?;

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
        let uses_azure_auth = endpoint.host_str().is_some_and(|host| {
            host == "dynamicsessions.io" || host.ends_with(".dynamicsessions.io")
        });

        Ok(Self {
            client: Client::new(),
            pool_management_endpoint: endpoint,
            uses_azure_auth,
        })
    }

    pub(crate) async fn execute(
        &self,
        identifier: &str,
        code: String,
        timeout_in_seconds: Option<u64>,
    ) -> Result<SessionOperationResult<PythonExecution>, AppError> {
        let envelope: SessionOperationResult<ExecutionEnvelope> = self
            .json_request(
                Method::POST,
                &["executions"],
                identifier,
                &[],
                Some(InlinePythonExecutionRequest {
                    code,
                    code_input_type: "Inline",
                    execution_type: "Synchronous",
                    timeout_in_seconds,
                }),
            )
            .await?;

        Ok(SessionOperationResult {
            metadata: envelope.metadata,
            value: PythonExecution {
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
        filename: String,
        content_type: Option<String>,
        content: Vec<u8>,
    ) -> Result<SessionOperationResult<SessionFile>, AppError> {
        let (directory, name) = match filename.rsplit_once('/') {
            Some((directory, name)) if !directory.is_empty() => (Some(directory), name),
            _ => (None, filename.as_str()),
        };
        let mut part = Part::bytes(content).file_name(name.to_string());
        if let Some(content_type) = content_type {
            part = part.mime_str(&content_type).map_err(|error| {
                AppError::request(format!("Invalid uploaded file content type: {error}"))
            })?;
        }
        let query_pairs = directory.map(|path| [("path", path)]);

        let request = self
            .data_plane_request(
                Method::POST,
                &["files"],
                identifier,
                query_pairs.as_ref().map_or(&[], |pairs| pairs.as_slice()),
            )
            .await?
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

    async fn json_request<T, B>(
        &self,
        method: Method,
        path_segments: &[&str],
        identifier: &str,
        query_pairs: &[(&str, &str)],
        body: Option<B>,
    ) -> Result<SessionOperationResult<T>, AppError>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        let mut request = self
            .data_plane_request(method, path_segments, identifier, query_pairs)
            .await?;
        if let Some(body) = body {
            request = request.json(&body);
        }
        parse_json_response(request, "call the session pool API").await
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
            DEFAULT_AZURE_SESSION_API_VERSION,
            query_pairs,
        );
        let request = self.client.request(method, url);
        if self.uses_azure_auth {
            return Ok(request.bearer_auth(data_plane_token().await?));
        }
        Ok(request)
    }
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: Option<String>,
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
struct InlinePythonExecutionRequest {
    code: String,
    code_input_type: &'static str,
    execution_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_in_seconds: Option<u64>,
}

async fn data_plane_token() -> Result<String, AppError> {
    let scope = format!("{DEFAULT_AZURE_SESSION_AUDIENCE}/.default");

    if let Ok(credential) = ManagedIdentityCredential::new(None)
        && let Ok(token) = credential.get_token(&[scope.as_str()], None).await
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
        .get_token(&[scope.as_str()], None)
        .await
        .map_err(|error| {
            AppError::internal_with_source(
                "Failed to acquire an Azure access token for session pool calls.".to_string(),
                error,
            )
        })?;
    Ok(token.token.secret().to_string())
}

fn session_pool_url(
    base_endpoint: &Url,
    path_segments: &[&str],
    identifier: &str,
    api_version: &str,
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
        query.append_pair("api-version", api_version);
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
    let response = builder.send().await.map_err(|error| {
        AppError::internal_with_source(format!("Failed to {operation}."), error)
    })?;
    let status = response.status();

    if !status.is_success() {
        let message = error_message(response).await?;
        return Err(map_session_pool_http_error(status, message));
    }

    let metadata = session_operation_metadata(response.headers());
    let value = response.json::<T>().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to decode the session-pool response while trying to {operation}."),
            error,
        )
    })?;

    Ok(SessionOperationResult { metadata, value })
}

fn session_operation_metadata(headers: &reqwest::header::HeaderMap) -> SessionOperationMetadata {
    let timings = RunTimings {
        allocation_time_ms: header_u64(headers, "x-ms-allocation-time"),
        container_execution_duration_ms: header_u64(headers, "x-ms-container-execution-duration"),
        overall_execution_time_ms: header_u64(headers, "x-ms-overall-execution-time"),
        preparation_time_ms: header_u64(headers, "x-ms-preparation-time"),
    };

    SessionOperationMetadata {
        operation_id: header_string(headers, "operation-id"),
        session_guid: header_string(headers, "x-ms-session-guid"),
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

fn header_string(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn header_u64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

async fn error_message(response: reqwest::Response) -> Result<String, AppError> {
    let bytes = response.bytes().await.map_err(|error| {
        AppError::internal_with_source(
            "Failed to read the session-pool error response.".to_string(),
            error,
        )
    })?;
    if let Ok(body) = serde_json::from_slice::<ErrorBody>(&bytes)
        && let Some(message) = body.message
    {
        return Ok(message);
    }

    let fallback = String::from_utf8_lossy(&bytes).trim().to_string();
    Ok(if fallback.is_empty() {
        "The session pool did not return an error message.".to_string()
    } else {
        fallback
    })
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

    use super::{map_session_pool_http_error, session_pool_url};

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
            "2025-10-02-preview",
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
            "2025-10-02-preview",
            &[("recursive", "true")],
        );

        assert_eq!(
            url.as_str(),
            "https://example.com/session-pool/files?identifier=cfg-123&api-version=2025-10-02-preview&recursive=true"
        );
    }
}
