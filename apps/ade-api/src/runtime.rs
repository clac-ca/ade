use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use azure_core::credentials::TokenCredential;
use azure_identity::{DeveloperToolsCredential, ManagedIdentityCredential};
use hmac::{Hmac, Mac};
use reqwest::{
    Client, Method, Url,
    header::HeaderMap,
    multipart::{Form, Part},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::info;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
};

const DEFAULT_ACTIVE_CONFIG_PACKAGE_NAME: &str = "ade-config";
const DEFAULT_ENGINE_PACKAGE_NAME: &str = "ade-engine";
const DEFAULT_RUNTIME_SESSION_SECRET: &str = "ade-local-session-secret";
const DEFAULT_AZURE_ARM_API_VERSION: &str = "2025-10-02-preview";
const DEFAULT_AZURE_SESSION_API_VERSION: &str = "2024-02-02-preview";
const DEFAULT_AZURE_MANAGEMENT_ENDPOINT: &str = "https://management.azure.com";
const DEFAULT_AZURE_SESSION_AUDIENCE: &str = "https://dynamicsessions.io";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonPackageArtifact {
    pub package_name: String,
    pub version: String,
    pub sha256: String,
    pub fingerprint: String,
    pub wheel_path: PathBuf,
}

impl PythonPackageArtifact {
    fn load_wheel_bytes(&self) -> Result<Vec<u8>, AppError> {
        fs::read(&self.wheel_path).map_err(|error| {
            AppError::io_with_source(
                format!(
                    "Failed to read the Python package wheel from '{}'.",
                    self.wheel_path.display()
                ),
                error,
            )
        })
    }

    fn wheel_filename(&self) -> Result<String, AppError> {
        self.wheel_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                AppError::config(format!(
                    "Python package wheel path '{}' does not end with a valid filename.",
                    self.wheel_path.display()
                ))
            })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRequest {
    pub properties: ExecutionProperties,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionProperties {
    pub code_input_type: String,
    pub execution_type: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonProxyResponse {
    pub body: Value,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BytesProxyResponse {
    pub body: Vec<u8>,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
}

pub struct RuntimeService {
    config_artifact: PythonPackageArtifact,
    backend: RuntimeBackend,
    engine_artifact: PythonPackageArtifact,
    job_session_identifier: String,
    mcp_api_key: Mutex<Option<String>>,
    mcp_endpoint: String,
    mcp_environment_id: Mutex<Option<String>>,
    pool_management_endpoint: String,
}

impl RuntimeService {
    pub fn create_from_env(env: &EnvBag, production: bool) -> Result<Option<Arc<Self>>, AppError> {
        let pool_management_endpoint =
            read_optional_trimmed_string(env, "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT");
        let Some(pool_management_endpoint) = pool_management_endpoint else {
            return Ok(None);
        };
        let mcp_endpoint = read_required_string(env, "ADE_SESSION_POOL_MCP_ENDPOINT")?;

        let config_artifact = resolve_active_config_artifact(env)?;
        let engine_artifact = resolve_engine_artifact(env)?;
        let secret = read_optional_trimmed_string(env, "ADE_RUNTIME_SESSION_SECRET")
            .or_else(|| (!production).then(|| DEFAULT_RUNTIME_SESSION_SECRET.to_string()))
            .ok_or_else(|| {
                AppError::config(
                    "Missing required environment variable: ADE_RUNTIME_SESSION_SECRET".to_string(),
                )
            })?;

        let backend = if read_optional_trimmed_string(env, "ADE_SESSION_POOL_RESOURCE_ID").is_some()
        {
            RuntimeBackend::Azure(AzureSessionPoolBackend::from_env(env)?)
        } else {
            RuntimeBackend::Local(LocalSessionPoolBackend::new())
        };

        Ok(Some(Arc::new(Self {
            config_artifact: config_artifact.clone(),
            backend,
            engine_artifact,
            job_session_identifier: derive_session_identifier(&secret, &config_artifact.fingerprint),
            mcp_api_key: Mutex::new(None),
            mcp_endpoint,
            mcp_environment_id: Mutex::new(None),
            pool_management_endpoint,
        })))
    }

    pub async fn execute(&self, request: ExecutionRequest) -> Result<JsonProxyResponse, AppError> {
        if request.properties.code_input_type != "inline" {
            return Err(AppError::request(
                "Only inline execution requests are supported.".to_string(),
            ));
        }
        if request.properties.execution_type != "synchronous" {
            return Err(AppError::request(
                "Only synchronous execution requests are supported.".to_string(),
            ));
        }

        self.ensure_required_wheels_uploaded().await?;
        self.data_plane_json(
            Method::POST,
            "code/execute",
            vec![],
            Some(json!({
                "properties": {
                    "codeInputType": "inline",
                    "executionType": "synchronous",
                    "code": wrap_execution_code(
                        &self.engine_artifact,
                        &self.config_artifact,
                        &request.properties.code,
                    )?,
                }
            })),
        )
        .await
    }

    pub async fn upload_file(
        &self,
        filename: String,
        content_type: Option<String>,
        content: Vec<u8>,
    ) -> Result<JsonProxyResponse, AppError> {
        let mut part = Part::bytes(content).file_name(filename);
        if let Some(content_type) = content_type {
            part = part.mime_str(&content_type).map_err(|error| {
                AppError::request(format!("Invalid uploaded file content type: {error}"))
            })?;
        }

        let form = Form::new().part("file", part);
        self.data_plane_multipart("files/upload", form).await
    }

    pub async fn list_files(&self) -> Result<JsonProxyResponse, AppError> {
        self.data_plane_json(Method::GET, "files", vec![], None)
            .await
    }

    pub async fn download_file(&self, filename: &str) -> Result<BytesProxyResponse, AppError> {
        self.data_plane_bytes(Method::GET, &format!("files/content/{filename}"), vec![])
            .await
    }

    pub async fn mcp(&self, request: Value) -> Result<JsonProxyResponse, AppError> {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::request("MCP request must include a method.".to_string()))?;

        match method {
            "initialize" | "tools/list" => self.mcp_request(request).await,
            "tools/call" => self.mcp_tools_call(request).await,
            _ => self.mcp_request(request).await,
        }
    }

    pub async fn stop_session(&self) -> Result<JsonProxyResponse, AppError> {
        *self.mcp_environment_id.lock().await = None;
        match self.backend {
            RuntimeBackend::Local(_) => {
                self.data_plane_json(
                    Method::POST,
                    ".management/stopSession",
                    vec![],
                    Some(json!({})),
                )
                .await
            }
            RuntimeBackend::Azure(_) => Err(AppError::request(
                "Azure built-in session pools do not expose a stopSession data-plane API."
                    .to_string(),
            )),
        }
    }

    fn mode_label(&self) -> &'static str {
        match self.backend {
            RuntimeBackend::Local(_) => "local session pool emulator",
            RuntimeBackend::Azure(_) => "Azure session pools",
        }
    }

    async fn ensure_required_wheels_uploaded(&self) -> Result<(), AppError> {
        let files = self.list_files().await?;
        for artifact in [&self.engine_artifact, &self.config_artifact] {
            let wheel_filename = artifact.wheel_filename()?;
            if session_contains_file(&files.body, &wheel_filename) {
                continue;
            }

            let wheel_bytes = artifact.load_wheel_bytes()?;
            let form = Form::new().part(
                "file",
                Part::bytes(wheel_bytes)
                    .file_name(wheel_filename)
                    .mime_str("application/octet-stream")
                    .expect("hard-coded content type is valid"),
            );
            let _ = self.data_plane_multipart("files/upload", form).await?;
        }
        Ok(())
    }

    async fn data_plane_json(
        &self,
        method: Method,
        path: &str,
        query: Vec<(&str, String)>,
        body: Option<Value>,
    ) -> Result<JsonProxyResponse, AppError> {
        let url = session_pool_url(
            &self.pool_management_endpoint,
            path,
            &self.job_session_identifier,
            self.backend.data_plane_api_version(),
            query,
        )?;
        let mut request = self
            .backend
            .data_plane_request(self.backend.client(), method, url)
            .await?;
        if let Some(body) = body {
            request = request.json(&body);
        }
        parse_json_response(request, "call the session pool API").await
    }

    async fn data_plane_bytes(
        &self,
        method: Method,
        path: &str,
        query: Vec<(&str, String)>,
    ) -> Result<BytesProxyResponse, AppError> {
        let url = session_pool_url(
            &self.pool_management_endpoint,
            path,
            &self.job_session_identifier,
            self.backend.data_plane_api_version(),
            query,
        )?;
        let request = self
            .backend
            .data_plane_request(self.backend.client(), method, url)
            .await?;
        parse_bytes_response(request, "download a session pool file").await
    }

    async fn data_plane_multipart(
        &self,
        path: &str,
        form: Form,
    ) -> Result<JsonProxyResponse, AppError> {
        let url = session_pool_url(
            &self.pool_management_endpoint,
            path,
            &self.job_session_identifier,
            self.backend.data_plane_api_version(),
            vec![],
        )?;
        let request = self
            .backend
            .data_plane_request(self.backend.client(), Method::POST, url)
            .await?
            .multipart(form);
        parse_json_response(request, "upload a session pool file").await
    }

    async fn mcp_request(&self, request: Value) -> Result<JsonProxyResponse, AppError> {
        let url = Url::parse(&self.mcp_endpoint).map_err(|error| {
            AppError::config_with_source("Invalid MCP endpoint URL.".to_string(), error)
        })?;
        let api_key = self.mcp_api_key().await?;
        let request = self
            .backend
            .mcp_request(self.backend.client(), url, api_key.as_deref(), request)
            .await?;
        parse_json_response(request, "call the MCP endpoint").await
    }

    async fn mcp_tools_call(&self, request: Value) -> Result<JsonProxyResponse, AppError> {
        let tool_name = request
            .get("params")
            .and_then(Value::as_object)
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::request("MCP tools/call request must include a tool name.".to_string())
            })?;

        if tool_name == "launchShell" {
            if let Some(environment_id) = self.mcp_environment_id.lock().await.clone() {
                return Ok(cached_launch_shell_response(&request, environment_id));
            }

            let response = self.mcp_request(request).await?;
            if let Some(environment_id) = extract_environment_id(&response.body) {
                *self.mcp_environment_id.lock().await = Some(environment_id);
            }
            return Ok(response);
        }

        if tool_name == "runShellCommandInRemoteEnvironment"
            || tool_name == "runPythonCodeInRemoteEnvironment"
        {
            let environment_id = self.ensure_mcp_environment().await?;
            let request = with_mcp_environment_id(request, &environment_id)?;
            let response = self.mcp_request(request.clone()).await?;
            if mcp_environment_is_invalid(&response.body) {
                *self.mcp_environment_id.lock().await = None;
                let environment_id = self.ensure_mcp_environment().await?;
                return self
                    .mcp_request(with_mcp_environment_id(request, &environment_id)?)
                    .await;
            }
            return Ok(response);
        }

        self.mcp_request(request).await
    }

    async fn ensure_mcp_environment(&self) -> Result<String, AppError> {
        if let Some(environment_id) = self.mcp_environment_id.lock().await.clone() {
            return Ok(environment_id);
        }

        let request = json!({
            "jsonrpc": "2.0",
            "id": "ade-launch-shell",
            "method": "tools/call",
            "params": {
                "name": "launchShell",
                "arguments": {}
            }
        });
        let response = self.mcp_request(request).await?;
        let environment_id = extract_environment_id(&response.body).ok_or_else(|| {
            AppError::internal("The MCP endpoint did not return an environmentId.".to_string())
        })?;
        *self.mcp_environment_id.lock().await = Some(environment_id.clone());
        Ok(environment_id)
    }

    async fn mcp_api_key(&self) -> Result<Option<String>, AppError> {
        if matches!(self.backend, RuntimeBackend::Local(_)) {
            return Ok(None);
        }

        let mut cache = self.mcp_api_key.lock().await;
        if let Some(api_key) = cache.clone() {
            return Ok(Some(api_key));
        }

        let api_key = match &self.backend {
            RuntimeBackend::Azure(backend) => backend.fetch_mcp_api_key().await?,
            RuntimeBackend::Local(_) => return Ok(None),
        };
        *cache = Some(api_key.clone());
        Ok(Some(api_key))
    }
}

fn resolve_active_config_artifact(env: &EnvBag) -> Result<PythonPackageArtifact, AppError> {
    resolve_package_artifact(
        env,
        "ADE_ACTIVE_CONFIG_PACKAGE_NAME",
        "ADE_ACTIVE_CONFIG_WHEEL_PATH",
        Some("ADE_ACTIVE_CONFIG_VERSION"),
        DEFAULT_ACTIVE_CONFIG_PACKAGE_NAME,
        "packages/ade-config",
    )
}

fn resolve_engine_artifact(env: &EnvBag) -> Result<PythonPackageArtifact, AppError> {
    resolve_package_artifact(
        env,
        "ADE_ENGINE_PACKAGE_NAME",
        "ADE_ENGINE_WHEEL_PATH",
        None,
        DEFAULT_ENGINE_PACKAGE_NAME,
        "packages/ade-engine",
    )
}

fn resolve_package_artifact(
    env: &EnvBag,
    package_name_env_name: &str,
    wheel_path_env_name: &str,
    version_env_name: Option<&str>,
    default_package_name: &str,
    package_dir_relative: &str,
) -> Result<PythonPackageArtifact, AppError> {
    let package_name = read_optional_trimmed_string(env, package_name_env_name)
        .unwrap_or_else(|| default_package_name.to_string());
    let package_name_normalized = package_name.replace('-', "_");
    let wheel_path =
        if let Some(path) = read_optional_trimmed_string(env, wheel_path_env_name) {
            PathBuf::from(path)
        } else if let Some(path) =
            newest_wheel_in_dir(Path::new("/app/python"), &package_name_normalized)?
        {
            path
        } else {
            build_local_package_wheel(package_dir_relative, &package_name_normalized)?
        };

    if !wheel_path.is_file() {
        return Err(AppError::config(format!(
            "Python package wheel not found: '{}'.",
            wheel_path.display()
        )));
    }

    let wheel_bytes = fs::read(&wheel_path).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to read Python package wheel '{}'.",
                wheel_path.display()
            ),
            error,
        )
    })?;
    let sha256 = hex_sha256(&wheel_bytes);
    let version = version_env_name
        .and_then(|name| read_optional_trimmed_string(env, name))
        .or_else(|| parse_wheel_version(&wheel_path, &package_name_normalized))
        .ok_or_else(|| {
            AppError::config(format!(
                "Unable to determine the package version from '{}'.",
                wheel_path.display()
            ))
        })?;

    Ok(PythonPackageArtifact {
        package_name: package_name.clone(),
        version: version.clone(),
        sha256: sha256.clone(),
        fingerprint: format!("{package_name}@{version}:{sha256}"),
        wheel_path,
    })
}

fn build_local_package_wheel(
    package_dir_relative: &str,
    package_name_normalized: &str,
) -> Result<PathBuf, AppError> {
    let repo_root = repo_root();
    let package_dir = repo_root.join(package_dir_relative);
    if !package_dir.is_dir() {
        return Err(AppError::config(format!(
            "No wheel was provided and the local Python package source was not found at '{}'.",
            package_dir.display()
        )));
    }

    let output = std::process::Command::new("uv")
        .args(["build", "--directory", package_dir_relative])
        .current_dir(&repo_root)
        .output()
        .map_err(|error| {
            AppError::config_with_source(
                format!("Failed to run `uv build --directory {package_dir_relative}`."),
                error,
            )
        })?;

    if !output.status.success() {
        return Err(AppError::config(format!(
            "Failed to build the Python package wheel: {}",
            output_message(&output)
        )));
    }

    newest_wheel_in_dir(&package_dir.join("dist"), package_name_normalized)?.ok_or_else(|| {
        AppError::config(format!(
            "The Python package wheel was not produced in '{}'.",
            package_dir.join("dist").display()
        ))
    })
}

fn newest_wheel_in_dir(
    directory: &Path,
    package_name_normalized: &str,
) -> Result<Option<PathBuf>, AppError> {
    if !directory.is_dir() {
        return Ok(None);
    }

    let prefix = format!("{package_name_normalized}-");
    let mut wheels = fs::read_dir(directory)
        .map_err(|error| {
            AppError::io_with_source(
                format!(
                    "Failed to read runtime wheel directory '{}'.",
                    directory.display()
                ),
                error,
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with(&prefix) && value.ends_with(".whl"))
        })
        .collect::<Vec<_>>();

    wheels.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    Ok(wheels.pop())
}

fn parse_wheel_version(wheel_path: &Path, package_name_normalized: &str) -> Option<String> {
    let file_name = wheel_path.file_name()?.to_str()?;
    let prefix = format!("{package_name_normalized}-");
    let remainder = file_name.strip_prefix(&prefix)?;
    let without_extension = remainder.strip_suffix(".whl")?;
    without_extension.split('-').next().map(ToOwned::to_owned)
}

#[derive(Clone)]
enum RuntimeBackend {
    Local(LocalSessionPoolBackend),
    Azure(AzureSessionPoolBackend),
}

impl RuntimeBackend {
    fn client(&self) -> &Client {
        match self {
            RuntimeBackend::Local(backend) => &backend.client,
            RuntimeBackend::Azure(backend) => &backend.client,
        }
    }

    fn data_plane_api_version(&self) -> &str {
        match self {
            RuntimeBackend::Local(backend) => &backend.data_plane_api_version,
            RuntimeBackend::Azure(backend) => &backend.data_plane_api_version,
        }
    }

    async fn data_plane_request(
        &self,
        client: &Client,
        method: Method,
        url: Url,
    ) -> Result<reqwest::RequestBuilder, AppError> {
        match self {
            RuntimeBackend::Local(_) => Ok(client.request(method, url)),
            RuntimeBackend::Azure(backend) => Ok(client
                .request(method, url)
                .bearer_auth(backend.data_plane_token().await?)),
        }
    }

    async fn mcp_request(
        &self,
        client: &Client,
        url: Url,
        api_key: Option<&str>,
        body: Value,
    ) -> Result<reqwest::RequestBuilder, AppError> {
        let body = serde_json::to_vec(&body).map_err(|error| {
            AppError::internal_with_source(
                "Failed to encode the MCP request body.".to_string(),
                error,
            )
        })?;
        match self {
            RuntimeBackend::Local(_) => Ok(client
                .post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)),
            RuntimeBackend::Azure(_) => {
                let api_key = api_key.ok_or_else(|| {
                    AppError::internal("MCP API key is not available.".to_string())
                })?;
                Ok(client
                    .post(url)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .header("x-ms-apikey", api_key)
                    .body(body))
            }
        }
    }
}

#[derive(Clone)]
struct LocalSessionPoolBackend {
    client: Client,
    data_plane_api_version: String,
}

impl LocalSessionPoolBackend {
    fn new() -> Self {
        Self {
            client: Client::new(),
            data_plane_api_version: DEFAULT_AZURE_SESSION_API_VERSION.to_string(),
        }
    }
}

#[derive(Clone)]
struct AzureSessionPoolBackend {
    arm_api_version: String,
    audience: String,
    client: Client,
    data_plane_api_version: String,
    management_endpoint: String,
    resource_id: String,
}

impl AzureSessionPoolBackend {
    fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        Ok(Self {
            arm_api_version: DEFAULT_AZURE_ARM_API_VERSION.to_string(),
            audience: DEFAULT_AZURE_SESSION_AUDIENCE.to_string(),
            client: Client::new(),
            data_plane_api_version: DEFAULT_AZURE_SESSION_API_VERSION.to_string(),
            management_endpoint: DEFAULT_AZURE_MANAGEMENT_ENDPOINT.to_string(),
            resource_id: read_required_string(env, "ADE_SESSION_POOL_RESOURCE_ID")?,
        })
    }

    async fn fetch_mcp_api_key(&self) -> Result<String, AppError> {
        let token = self.arm_token().await?;
        let url = self.arm_resource_url("fetchMcpServerCredentials")?;
        let response = self
            .client
            .post(url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|error| {
                AppError::internal_with_source(
                    "Failed to fetch MCP credentials for the Azure session pool.".to_string(),
                    error,
                )
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let message = error_message(response).await?;
            return Err(map_runtime_http_error(status, message));
        }

        let credentials: McpCredentials =
            parse_json_body(response, "read the Azure MCP credentials").await?;
        Ok(credentials.api_key)
    }

    async fn arm_token(&self) -> Result<String, AppError> {
        token_for_scope("https://management.azure.com").await
    }

    async fn data_plane_token(&self) -> Result<String, AppError> {
        token_for_scope(&self.audience).await
    }

    fn arm_resource_url(&self, relative_path: &str) -> Result<Url, AppError> {
        let mut url = Url::parse(&format!(
            "{}{}{}{}",
            self.management_endpoint.trim_end_matches('/'),
            self.resource_id,
            if relative_path.is_empty() { "" } else { "/" },
            relative_path.trim_start_matches('/'),
        ))
        .map_err(|error| {
            AppError::config_with_source(
                "Azure management endpoint configuration is invalid.".to_string(),
                error,
            )
        })?;
        url.query_pairs_mut()
            .append_pair("api-version", &self.arm_api_version);
        Ok(url)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpCredentials {
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

fn derive_session_identifier(secret: &str, fingerprint: &str) -> String {
    type HmacSha256 = Hmac<Sha256>;

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts arbitrary key lengths");
    mac.update(fingerprint.as_bytes());
    let digest = hex::encode(mac.finalize().into_bytes());
    format!("cfg-{}", &digest[..32])
}

fn wrap_execution_code(
    engine_artifact: &PythonPackageArtifact,
    config_artifact: &PythonPackageArtifact,
    user_code: &str,
) -> Result<String, AppError> {
    let engine_wheel_filename = engine_artifact.wheel_filename()?;
    let config_wheel_filename = config_artifact.wheel_filename()?;
    Ok(format!(
        concat!(
            "import importlib.metadata\n",
            "import subprocess\n",
            "import sys\n",
            "\n",
            "ENGINE_PACKAGE = {engine_package_name:?}\n",
            "ENGINE_VERSION = {engine_version:?}\n",
            "ENGINE_WHEEL_PATH = {engine_wheel_path:?}\n",
            "CONFIG_PACKAGE = {config_package_name:?}\n",
            "CONFIG_VERSION = {config_version:?}\n",
            "CONFIG_WHEEL_PATH = {config_wheel_path:?}\n",
            "\n",
            "try:\n",
            "    installed_engine_version = importlib.metadata.version(ENGINE_PACKAGE)\n",
            "except importlib.metadata.PackageNotFoundError:\n",
            "    installed_engine_version = None\n",
            "\n",
            "if installed_engine_version != ENGINE_VERSION:\n",
            "    subprocess.run(\n",
            "        [sys.executable, '-m', 'pip', 'install', '--force-reinstall', ENGINE_WHEEL_PATH],\n",
            "        check=True,\n",
            "    )\n",
            "\n",
            "try:\n",
            "    installed_config_version = importlib.metadata.version(CONFIG_PACKAGE)\n",
            "except importlib.metadata.PackageNotFoundError:\n",
            "    installed_config_version = None\n",
            "\n",
            "if installed_config_version != CONFIG_VERSION:\n",
            "    subprocess.run(\n",
            "        [sys.executable, '-m', 'pip', 'install', '--no-deps', '--force-reinstall', CONFIG_WHEEL_PATH],\n",
            "        check=True,\n",
            "    )\n",
            "\n",
            "{user_code}\n"
        ),
        engine_package_name = engine_artifact.package_name,
        engine_version = engine_artifact.version,
        engine_wheel_path = format!("/mnt/data/{engine_wheel_filename}"),
        config_package_name = config_artifact.package_name,
        config_version = config_artifact.version,
        config_wheel_path = format!("/mnt/data/{config_wheel_filename}"),
        user_code = user_code
    ))
}

async fn token_for_scope(scope_base: &str) -> Result<String, AppError> {
    let scope = format!("{}/.default", scope_base.trim_end_matches('/'));

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
                "Failed to acquire an Azure access token for session runtime calls.".to_string(),
                error,
            )
        })?;
    Ok(token.token.secret().to_string())
}

fn session_pool_url(
    base_endpoint: &str,
    path: &str,
    identifier: &str,
    api_version: &str,
    extra_query: Vec<(&str, String)>,
) -> Result<Url, AppError> {
    let mut url = Url::parse(&format!(
        "{}/{}",
        base_endpoint.trim_end_matches('/'),
        path.trim_start_matches('/'),
    ))
    .map_err(|error| {
        AppError::config_with_source(
            "Session pool endpoint is not a valid URL.".to_string(),
            error,
        )
    })?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("identifier", identifier);
        query.append_pair("api-version", api_version);
        for (key, value) in extra_query {
            query.append_pair(key, &value);
        }
    }
    Ok(url)
}

fn extract_environment_id(body: &Value) -> Option<String> {
    body.get("result")
        .and_then(|value| value.get("structuredContent"))
        .and_then(|value| value.get("environmentId"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn session_contains_file(body: &Value, filename: &str) -> bool {
    body.get("value")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .get("properties")
                    .and_then(Value::as_object)
                    .and_then(|properties| properties.get("filename"))
                    .and_then(Value::as_str)
                    == Some(filename)
            })
        })
}

fn cached_launch_shell_response(request: &Value, environment_id: String) -> JsonProxyResponse {
    JsonProxyResponse {
        body: json!({
            "jsonrpc": "2.0",
            "id": request.get("id").cloned().unwrap_or(Value::Null),
            "result": {
                "structuredContent": {
                    "environmentId": environment_id
                }
            }
        }),
        headers: Vec::new(),
    }
}

fn with_mcp_environment_id(mut request: Value, environment_id: &str) -> Result<Value, AppError> {
    let params = request
        .get_mut("params")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            AppError::request("MCP tools/call request must include params.".to_string())
        })?;
    let arguments = params
        .entry("arguments")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| AppError::request("MCP tool arguments must be an object.".to_string()))?;
    arguments.insert(
        "environmentId".to_string(),
        Value::String(environment_id.to_string()),
    );
    Ok(request)
}

fn mcp_environment_is_invalid(body: &Value) -> bool {
    body.get("error")
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .map(|message| {
            let lower = message.to_ascii_lowercase();
            lower.contains("environment")
                && (lower.contains("not found") || lower.contains("invalid"))
        })
        .unwrap_or(false)
}

async fn parse_json_response(
    builder: reqwest::RequestBuilder,
    operation: &str,
) -> Result<JsonProxyResponse, AppError> {
    let response = builder.send().await.map_err(|error| {
        AppError::internal_with_source(format!("Failed to {operation}."), error)
    })?;
    let status = response.status();
    let headers = forwarded_headers(response.headers());

    if !status.is_success() {
        let message = error_message(response).await?;
        return Err(map_runtime_http_error(status, message));
    }

    let body = response.json::<Value>().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to decode the runtime response while trying to {operation}."),
            error,
        )
    })?;

    Ok(JsonProxyResponse { body, headers })
}

async fn parse_bytes_response(
    builder: reqwest::RequestBuilder,
    operation: &str,
) -> Result<BytesProxyResponse, AppError> {
    let response = builder.send().await.map_err(|error| {
        AppError::internal_with_source(format!("Failed to {operation}."), error)
    })?;
    let status = response.status();
    let headers = forwarded_headers(response.headers());

    if !status.is_success() {
        let message = error_message(response).await?;
        return Err(map_runtime_http_error(status, message));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let body = response.bytes().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to read the runtime response while trying to {operation}."),
            error,
        )
    })?;

    Ok(BytesProxyResponse {
        body: body.to_vec(),
        content_type,
        headers,
    })
}

async fn parse_json_body<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    operation: &str,
) -> Result<T, AppError> {
    response.json::<T>().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to decode the response while trying to {operation}."),
            error,
        )
    })
}

fn forwarded_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let header_name = name.as_str().to_ascii_lowercase();
            let should_forward = header_name == "operation-id"
                || header_name == "operation-location"
                || header_name == "x-ms-session-guid"
                || header_name.starts_with("x-ms-");
            if !should_forward {
                return None;
            }
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn read_required_string(env: &EnvBag, name: &str) -> Result<String, AppError> {
    read_optional_trimmed_string(env, name)
        .ok_or_else(|| AppError::config(format!("Missing required environment variable: {name}")))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

async fn error_message(response: reqwest::Response) -> Result<String, AppError> {
    let bytes = response.bytes().await.map_err(|error| {
        AppError::internal_with_source(
            "Failed to read the runtime error response.".to_string(),
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
        "The runtime did not return an error message.".to_string()
    } else {
        fallback
    })
}

fn map_runtime_http_error(status: reqwest::StatusCode, message: String) -> AppError {
    match status {
        reqwest::StatusCode::NOT_FOUND => AppError::not_found(message),
        reqwest::StatusCode::BAD_REQUEST
        | reqwest::StatusCode::UNPROCESSABLE_ENTITY
        | reqwest::StatusCode::CONFLICT => AppError::request(message),
        reqwest::StatusCode::SERVICE_UNAVAILABLE => AppError::unavailable(message),
        _ => AppError::status(status, message),
    }
}

fn hex_sha256(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

fn output_message(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout,
        (true, false) => stderr,
        (true, true) => format!("process exited with status {}", output.status),
    }
}

pub fn log_runtime_mode(service: &Option<Arc<RuntimeService>>) {
    if let Some(service) = service {
        info!(
            "Hosted ADE runtime is enabled using {}.",
            service.mode_label()
        );
    } else {
        info!("Hosted ADE runtime is disabled.");
    }
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;
    use reqwest::StatusCode as ReqwestStatusCode;
    use reqwest::header::{HeaderMap, HeaderValue};
    use serde_json::json;

    use super::{
        cached_launch_shell_response, derive_session_identifier, forwarded_headers,
        map_runtime_http_error, mcp_environment_is_invalid, session_contains_file,
        with_mcp_environment_id,
    };

    #[test]
    fn derives_stable_session_identifiers() {
        let first = derive_session_identifier("secret", "ade-config@0.1.0:abc");
        let second = derive_session_identifier("secret", "ade-config@0.1.0:abc");

        assert_eq!(first, second);
        assert!(first.starts_with("cfg-"));
    }

    #[test]
    fn runtime_http_errors_preserve_upstream_status_codes() {
        let response = map_runtime_http_error(
            ReqwestStatusCode::TOO_MANY_REQUESTS,
            "Too many requests".to_string(),
        )
        .into_response();

        assert_eq!(response.status().as_u16(), 429);
    }

    #[test]
    fn cached_launch_shell_response_reuses_existing_environment() {
        let response = cached_launch_shell_response(&json!({"id": "1"}), "env-123".to_string());

        assert_eq!(
            response.body["result"]["structuredContent"]["environmentId"],
            "env-123"
        );
    }

    #[test]
    fn session_contains_file_matches_azure_style_file_lists() {
        let body = json!({
            "value": [
                {
                    "properties": {
                        "filename": "ade_config-0.1.0-py3-none-any.whl"
                    }
                }
            ]
        });

        assert!(session_contains_file(
            &body,
            "ade_config-0.1.0-py3-none-any.whl"
        ));
        assert!(!session_contains_file(&body, "input.csv"));
    }

    #[test]
    fn mcp_environment_id_is_injected_server_side() {
        let request = with_mcp_environment_id(
            json!({
                "method": "tools/call",
                "params": {
                    "name": "runPythonCodeInRemoteEnvironment",
                    "arguments": {
                        "pythonCode": "print('hi')"
                    }
                }
            }),
            "env-123",
        )
        .unwrap();

        assert_eq!(request["params"]["arguments"]["environmentId"], "env-123");
    }

    #[test]
    fn invalid_environment_errors_trigger_relaunch() {
        assert!(mcp_environment_is_invalid(&json!({
            "error": {
                "message": "Environment not found: env-123"
            }
        })));
    }

    #[test]
    fn forwards_selected_runtime_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("operation-id", HeaderValue::from_static("abc"));
        headers.insert("x-ms-session-guid", HeaderValue::from_static("cfg-1"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        assert_eq!(
            forwarded_headers(&headers),
            vec![
                ("operation-id".to_string(), "abc".to_string()),
                ("x-ms-session-guid".to_string(), "cfg-1".to_string()),
            ]
        );
    }
}
