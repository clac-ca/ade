use azure_core::credentials::TokenCredential;
use azure_identity::{DeveloperToolsCredential, ManagedIdentityCredential};
use reqwest::{
    Client, Method, Url,
    multipart::{Form, Part},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
};

const DEFAULT_AZURE_SESSION_API_VERSION: &str = "2025-10-02-preview";
const DEFAULT_AZURE_SESSION_AUDIENCE: &str = "https://dynamicsessions.io";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionFile {
    pub(crate) filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_modified_time: Option<String>,
    pub(crate) size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PythonExecution {
    pub(crate) duration_ms: u64,
    pub(crate) status: String,
    pub(crate) stderr: String,
    pub(crate) stdout: String,
}

pub(crate) struct SessionPoolClient {
    client: Client,
    pool_management_endpoint: String,
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

        let endpoint = Url::parse(&pool_management_endpoint).map_err(|error| {
            AppError::config_with_source(
                "Session pool endpoint is not a valid URL.".to_string(),
                error,
            )
        })?;
        let uses_azure_auth = endpoint.host_str().is_some_and(is_dynamicsessions_host);

        Ok(Self {
            client: Client::new(),
            pool_management_endpoint,
            uses_azure_auth,
        })
    }

    pub(crate) async fn execute(
        &self,
        identifier: &str,
        code: String,
        timeout_in_seconds: Option<u64>,
    ) -> Result<PythonExecution, AppError> {
        let envelope: ExecutionEnvelope = self
            .json_request(
                Method::POST,
                &["executions"],
                identifier,
                Some(InlinePythonExecutionRequest {
                    code,
                    code_input_type: "Inline",
                    execution_type: "Synchronous",
                    timeout_in_seconds,
                }),
            )
            .await?;

        Ok(PythonExecution {
            duration_ms: envelope.result.execution_time_in_milliseconds.unwrap_or(0),
            status: envelope.status,
            stderr: envelope.result.stderr,
            stdout: envelope.result.stdout,
        })
    }

    pub(crate) async fn upload_file(
        &self,
        identifier: &str,
        filename: String,
        content_type: Option<String>,
        content: Vec<u8>,
    ) -> Result<SessionFile, AppError> {
        let mut part = Part::bytes(content).file_name(filename);
        if let Some(content_type) = content_type {
            part = part.mime_str(&content_type).map_err(|error| {
                AppError::request(format!("Invalid uploaded file content type: {error}"))
            })?;
        }

        let request = self
            .data_plane_request(Method::POST, &["files"], identifier)
            .await?
            .multipart(Form::new().part("file", part));
        let record: AzureFileRecord =
            parse_json_response(request, "upload a session pool file").await?;
        Ok(record.into_session_file())
    }

    pub(crate) async fn list_files(&self, identifier: &str) -> Result<Vec<SessionFile>, AppError> {
        let envelope: FilesEnvelope = self
            .json_request(Method::GET, &["files"], identifier, None::<()>)
            .await?;
        Ok(envelope
            .value
            .into_iter()
            .map(AzureFileRecord::into_session_file)
            .collect())
    }

    pub(crate) async fn download_file(
        &self,
        identifier: &str,
        filename: &str,
    ) -> Result<(String, Vec<u8>), AppError> {
        let mut path_segments = vec!["files"];
        path_segments.extend(filename.split('/').filter(|segment| !segment.is_empty()));
        path_segments.push("content");
        let request = self
            .data_plane_request(Method::GET, &path_segments, identifier)
            .await?;
        parse_bytes_response(request, "download a session pool file").await
    }

    async fn json_request<T, B>(
        &self,
        method: Method,
        path_segments: &[&str],
        identifier: &str,
        body: Option<B>,
    ) -> Result<T, AppError>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        let mut request = self
            .data_plane_request(method, path_segments, identifier)
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
    ) -> Result<reqwest::RequestBuilder, AppError> {
        let url = session_pool_url(
            &self.pool_management_endpoint,
            path_segments,
            identifier,
            DEFAULT_AZURE_SESSION_API_VERSION,
        )?;
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

#[derive(Deserialize)]
struct FilesEnvelope {
    value: Vec<AzureFileRecord>,
}

impl AzureFileRecord {
    fn into_session_file(self) -> SessionFile {
        let filename = match self.directory.as_deref() {
            Some("") | Some(".") | None => self.name,
            Some(directory) => format!("{directory}/{}", self.name),
        };

        SessionFile {
            filename,
            last_modified_time: self.last_modified_at,
            size: self.size_in_bytes,
        }
    }
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
    let scope = format!("{}/.default", DEFAULT_AZURE_SESSION_AUDIENCE);

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

fn is_dynamicsessions_host(host: &str) -> bool {
    host == "dynamicsessions.io" || host.ends_with(".dynamicsessions.io")
}

fn session_pool_url(
    base_endpoint: &str,
    path_segments: &[&str],
    identifier: &str,
    api_version: &str,
) -> Result<Url, AppError> {
    let mut url =
        Url::parse(&format!("{}/", base_endpoint.trim_end_matches('/'))).map_err(|error| {
            AppError::config_with_source(
                "Session pool endpoint is not a valid URL.".to_string(),
                error,
            )
        })?;
    {
        let mut segments = url.path_segments_mut().map_err(|()| {
            AppError::config("Session pool endpoint cannot be used as a base URL.".to_string())
        })?;
        segments.pop_if_empty();
        for segment in path_segments {
            segments.push(segment);
        }
    }
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("identifier", identifier);
        query.append_pair("api-version", api_version);
    }
    Ok(url)
}

async fn parse_json_response<T>(
    builder: reqwest::RequestBuilder,
    operation: &str,
) -> Result<T, AppError>
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

    response.json::<T>().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to decode the session-pool response while trying to {operation}."),
            error,
        )
    })
}

async fn parse_bytes_response(
    builder: reqwest::RequestBuilder,
    operation: &str,
) -> Result<(String, Vec<u8>), AppError> {
    let response = builder.send().await.map_err(|error| {
        AppError::internal_with_source(format!("Failed to {operation}."), error)
    })?;
    let status = response.status();

    if !status.is_success() {
        let message = error_message(response).await?;
        return Err(map_session_pool_http_error(status, message));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let body = response.bytes().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to read the session-pool response while trying to {operation}."),
            error,
        )
    })?;

    Ok((content_type, body.to_vec()))
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
    use reqwest::StatusCode as ReqwestStatusCode;

    use super::{is_dynamicsessions_host, map_session_pool_http_error, session_pool_url};

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
            "https://example.com/session-pool",
            &[
                "files",
                "runs",
                "run 1",
                "output",
                "input #1.xlsx",
                "content",
            ],
            "cfg-123",
            "2025-10-02-preview",
        )
        .unwrap();

        assert_eq!(
            url.as_str(),
            "https://example.com/session-pool/files/runs/run%201/output/input%20%231.xlsx/content?identifier=cfg-123&api-version=2025-10-02-preview"
        );
    }

    #[test]
    fn auth_is_inferred_from_dynamicsessions_hosts() {
        assert!(is_dynamicsessions_host("canadacentral.dynamicsessions.io"));
        assert!(is_dynamicsessions_host("dynamicsessions.io"));
        assert!(!is_dynamicsessions_host("127.0.0.1"));
    }
}
