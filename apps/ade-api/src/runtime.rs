use std::{
    fs,
    path::{Path, PathBuf},
    process::Output,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use azure_core::credentials::TokenCredential;
use azure_identity::{DeveloperToolsCredential, ManagedIdentityCredential};
use hmac::{Hmac, Mac};
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{process::Command, sync::Mutex, time::sleep};
use tracing::info;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
};

const DEFAULT_ACTIVE_CONFIG_PACKAGE_NAME: &str = "ade-config";
const DEFAULT_DOCKER_AGENT_HOST: &str = "127.0.0.1";
const DEFAULT_DOCKER_AGENT_PORT: u16 = 9100;
const DEFAULT_DOCKER_COMMAND: &str = "docker";
const DEFAULT_DOCKER_IMAGE: &str = "ade-sandbox:local";
const DEFAULT_DOCKER_CONTAINER_PREFIX: &str = "ade-sandbox";
const DEFAULT_EVENT_WAIT_MS: u64 = 15_000;
const DEFAULT_RUNTIME_SESSION_SECRET: &str = "ade-local-session-secret";
const DEFAULT_SANDBOX_AGENT_PORT: u16 = 9000;
const DEFAULT_AZURE_SESSION_API_VERSION: &str = "2025-10-02-preview";
const DEFAULT_AZURE_SESSION_AUDIENCE: &str = "https://dynamicsessions.io";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveConfigArtifact {
    pub package_name: String,
    pub version: String,
    pub sha256: String,
    pub fingerprint: String,
    pub wheel_path: PathBuf,
}

impl ActiveConfigArtifact {
    fn load_wheel_bytes(&self) -> Result<Vec<u8>, AppError> {
        fs::read(&self.wheel_path).map_err(|error| {
            AppError::io_with_source(
                format!(
                    "Failed to read the active config wheel from '{}'.",
                    self.wheel_path.display()
                ),
                error,
            )
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstalledConfigStatus {
    pub package_name: String,
    pub version: String,
    pub sha256: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TerminalStatus {
    pub open: bool,
    pub cwd: String,
    pub cols: Option<i32>,
    pub rows: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RunStatus {
    pub active: bool,
    pub input_path: Option<String>,
    pub output_path: Option<String>,
    pub pid: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub workspace: String,
    pub installed_config: Option<InstalledConfigStatus>,
    pub terminal: TerminalStatus,
    pub run: RunStatus,
    pub earliest_seq: u64,
    pub latest_seq: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RuntimeEvent {
    pub seq: u64,
    pub time: u64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventBatch {
    pub needs_resync: bool,
    pub events: Vec<RuntimeEvent>,
    pub status: RuntimeStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UploadedFile {
    pub path: String,
    pub size: usize,
}

#[async_trait]
pub trait SessionRuntime: Send + Sync {
    async fn ensure_session(&self, session_identifier: &str) -> Result<(), AppError>;
    async fn status(&self, session_identifier: &str) -> Result<RuntimeStatus, AppError>;
    async fn install_config(
        &self,
        session_identifier: &str,
        artifact: &ActiveConfigArtifact,
    ) -> Result<RuntimeStatus, AppError>;
    async fn upload_file(
        &self,
        session_identifier: &str,
        relative_path: &str,
        content: Vec<u8>,
    ) -> Result<UploadedFile, AppError>;
    async fn download_file(
        &self,
        session_identifier: &str,
        relative_path: &str,
    ) -> Result<Vec<u8>, AppError>;
    async fn rpc(
        &self,
        session_identifier: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, AppError>;
    async fn poll_events(
        &self,
        session_identifier: &str,
        after: u64,
        wait_ms: u64,
    ) -> Result<EventBatch, AppError>;
    async fn reset_session(&self, session_identifier: &str) -> Result<(), AppError>;
}

pub struct RuntimeService {
    runtime: Arc<dyn SessionRuntime>,
    artifact: ActiveConfigArtifact,
    session_identifier: String,
    ensure_lock: Mutex<()>,
}

impl RuntimeService {
    pub fn new(
        runtime: Arc<dyn SessionRuntime>,
        artifact: ActiveConfigArtifact,
        session_identifier: String,
    ) -> Self {
        Self {
            runtime,
            artifact,
            session_identifier,
            ensure_lock: Mutex::new(()),
        }
    }

    pub fn create_from_env(env: &EnvBag, production: bool) -> Result<Option<Arc<Self>>, AppError> {
        let runtime_mode = resolve_runtime_mode(env)?;
        let Some(runtime_mode) = runtime_mode else {
            return Ok(None);
        };

        let artifact = resolve_active_config_artifact(env)?;
        let secret = read_optional_trimmed_string(env, "ADE_RUNTIME_SESSION_SECRET")
            .or_else(|| (!production).then(|| DEFAULT_RUNTIME_SESSION_SECRET.to_string()))
            .ok_or_else(|| {
                AppError::config(
                    "Missing required environment variable: ADE_RUNTIME_SESSION_SECRET".to_string(),
                )
            })?;
        let session_identifier = derive_session_identifier(&secret, &artifact.fingerprint);

        let runtime: Arc<dyn SessionRuntime> = match runtime_mode {
            RuntimeMode::Docker => Arc::new(DockerSessionRuntime::from_env(env)?),
            RuntimeMode::Azure => Arc::new(AzureSessionRuntime::from_env(env)?),
        };

        Ok(Some(Arc::new(Self::new(
            runtime,
            artifact,
            session_identifier,
        ))))
    }

    pub async fn ensure_ready(&self) -> Result<RuntimeStatus, AppError> {
        let _guard = self.ensure_lock.lock().await;
        self.runtime
            .ensure_session(&self.session_identifier)
            .await?;

        let status = self.runtime.status(&self.session_identifier).await?;
        if status
            .installed_config
            .as_ref()
            .map(|value| value.fingerprint.as_str())
            == Some(self.artifact.fingerprint.as_str())
        {
            return Ok(status);
        }

        self.runtime
            .install_config(&self.session_identifier, &self.artifact)
            .await
    }

    pub async fn upload_file(
        &self,
        relative_path: &str,
        content: Vec<u8>,
    ) -> Result<UploadedFile, AppError> {
        let _ = self.ensure_ready().await?;
        self.runtime
            .upload_file(&self.session_identifier, relative_path, content)
            .await
    }

    pub async fn download_file(&self, relative_path: &str) -> Result<Vec<u8>, AppError> {
        let _ = self.ensure_ready().await?;
        self.runtime
            .download_file(&self.session_identifier, relative_path)
            .await
    }

    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value, AppError> {
        let _ = self.ensure_ready().await?;
        self.runtime
            .rpc(&self.session_identifier, method, params)
            .await
    }

    pub async fn poll_events(&self, after: u64, wait_ms: u64) -> Result<EventBatch, AppError> {
        self.runtime
            .poll_events(&self.session_identifier, after, wait_ms)
            .await
    }

    pub async fn reset_session(&self) -> Result<(), AppError> {
        let _guard = self.ensure_lock.lock().await;
        self.runtime.reset_session(&self.session_identifier).await
    }

    pub fn default_event_wait_ms(&self) -> u64 {
        DEFAULT_EVENT_WAIT_MS
    }
}

#[derive(Clone, Copy)]
enum RuntimeMode {
    Docker,
    Azure,
}

fn resolve_runtime_mode(env: &EnvBag) -> Result<Option<RuntimeMode>, AppError> {
    let value = read_optional_trimmed_string(env, "ADE_RUNTIME_MODE").or_else(|| {
        read_optional_trimmed_string(env, "ADE_AZURE_SESSION_POOL_ENDPOINT")
            .map(|_| "azure".to_string())
    });

    let Some(value) = value else {
        return Ok(None);
    };

    match value.as_str() {
        "docker" => Ok(Some(RuntimeMode::Docker)),
        "azure" => Ok(Some(RuntimeMode::Azure)),
        _ => Err(AppError::config(format!(
            "Unsupported ADE_RUNTIME_MODE '{value}'. Expected 'docker' or 'azure'."
        ))),
    }
}

fn resolve_active_config_artifact(env: &EnvBag) -> Result<ActiveConfigArtifact, AppError> {
    let package_name = read_optional_trimmed_string(env, "ADE_ACTIVE_CONFIG_PACKAGE_NAME")
        .unwrap_or_else(|| DEFAULT_ACTIVE_CONFIG_PACKAGE_NAME.to_string());
    let package_name_normalized = package_name.replace('-', "_");
    let wheel_path =
        if let Some(path) = read_optional_trimmed_string(env, "ADE_ACTIVE_CONFIG_WHEEL_PATH") {
            PathBuf::from(path)
        } else if let Some(path) =
            newest_wheel_in_dir(Path::new("/app/python"), &package_name_normalized)?
        {
            path
        } else {
            build_local_active_config_wheel(&package_name_normalized)?
        };

    if !wheel_path.is_file() {
        return Err(AppError::config(format!(
            "Active config wheel not found: '{}'.",
            wheel_path.display()
        )));
    }

    let wheel_bytes = fs::read(&wheel_path).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to read active config wheel '{}'.",
                wheel_path.display()
            ),
            error,
        )
    })?;
    let sha256 = hex_sha256(&wheel_bytes);
    let version = read_optional_trimmed_string(env, "ADE_ACTIVE_CONFIG_VERSION")
        .or_else(|| parse_wheel_version(&wheel_path, &package_name_normalized))
        .ok_or_else(|| {
            AppError::config(format!(
                "Unable to determine the active config version from '{}'.",
                wheel_path.display()
            ))
        })?;

    Ok(ActiveConfigArtifact {
        package_name: package_name.clone(),
        version: version.clone(),
        sha256: sha256.clone(),
        fingerprint: format!("{package_name}@{version}:{sha256}"),
        wheel_path,
    })
}

fn build_local_active_config_wheel(package_name_normalized: &str) -> Result<PathBuf, AppError> {
    let repo_root = repo_root();
    let package_dir = repo_root.join("packages/ade-config");
    if !package_dir.is_dir() {
        return Err(AppError::config(
            "No active config wheel was provided and the local ADE config package source was not found."
                .to_string(),
        ));
    }

    let output = std::process::Command::new("uv")
        .args(["build", "--directory", "packages/ade-config"])
        .current_dir(&repo_root)
        .output()
        .map_err(|error| {
            AppError::config_with_source(
                "Failed to run `uv build --directory packages/ade-config`.".to_string(),
                error,
            )
        })?;

    if !output.status.success() {
        return Err(AppError::config(format!(
            "Failed to build the active config wheel: {}",
            output_message(&output)
        )));
    }

    newest_wheel_in_dir(&package_dir.join("dist"), package_name_normalized)?.ok_or_else(|| {
        AppError::config(
            "The active config wheel was not produced in packages/ade-config/dist.".to_string(),
        )
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

fn derive_session_identifier(secret: &str, fingerprint: &str) -> String {
    type HmacSha256 = Hmac<Sha256>;

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts arbitrary key lengths");
    mac.update(fingerprint.as_bytes());
    let digest = mac.finalize().into_bytes();
    format!("cfg-{}", hex::encode(digest))
}

#[derive(Clone)]
struct DockerSessionRuntime {
    client: Client,
    docker_command: String,
    image: String,
    container_name_prefix: String,
    agent_host: String,
    agent_port: u16,
    repo_root: PathBuf,
    sandbox_dockerfile: PathBuf,
}

impl DockerSessionRuntime {
    fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let repo_root = repo_root();
        let sandbox_dockerfile = read_optional_trimmed_string(env, "ADE_RUNTIME_DOCKERFILE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root.join("packages/ade-engine/Dockerfile.sandbox"));

        Ok(Self {
            client: Client::new(),
            docker_command: read_optional_trimmed_string(env, "ADE_RUNTIME_DOCKER_COMMAND")
                .unwrap_or_else(|| DEFAULT_DOCKER_COMMAND.to_string()),
            image: read_optional_trimmed_string(env, "ADE_RUNTIME_DOCKER_IMAGE")
                .unwrap_or_else(|| DEFAULT_DOCKER_IMAGE.to_string()),
            container_name_prefix: read_optional_trimmed_string(
                env,
                "ADE_RUNTIME_DOCKER_CONTAINER_NAME_PREFIX",
            )
            .unwrap_or_else(|| DEFAULT_DOCKER_CONTAINER_PREFIX.to_string()),
            agent_host: read_optional_trimmed_string(env, "ADE_RUNTIME_DOCKER_AGENT_HOST")
                .unwrap_or_else(|| DEFAULT_DOCKER_AGENT_HOST.to_string()),
            agent_port: read_optional_trimmed_string(env, "ADE_RUNTIME_DOCKER_AGENT_PORT")
                .map(|value| {
                    value.parse().map_err(|error| {
                        AppError::config_with_source(
                            "ADE_RUNTIME_DOCKER_AGENT_PORT must be a valid port.".to_string(),
                            error,
                        )
                    })
                })
                .transpose()?
                .unwrap_or(DEFAULT_DOCKER_AGENT_PORT),
            repo_root,
            sandbox_dockerfile,
        })
    }

    fn container_name(&self, session_identifier: &str) -> String {
        let suffix = &session_identifier[..std::cmp::min(session_identifier.len(), 16)];
        format!("{}-{suffix}", self.container_name_prefix)
    }

    fn agent_url(&self) -> String {
        format!("http://{}:{}", self.agent_host, self.agent_port)
    }

    async fn docker_output(&self, args: Vec<String>) -> Result<Output, AppError> {
        Command::new(&self.docker_command)
            .args(&args)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|error| {
                AppError::internal_with_source(
                    format!(
                        "Failed to run the Docker command '{} {}'.",
                        self.docker_command,
                        args.join(" ")
                    ),
                    error,
                )
            })
    }

    async fn container_is_running(&self, container_name: &str) -> Result<bool, AppError> {
        let output = self
            .docker_output(vec![
                "container".to_string(),
                "inspect".to_string(),
                "--format".to_string(),
                "{{.State.Running}}".to_string(),
                container_name.to_string(),
            ])
            .await?;

        if !output.status.success() {
            return Ok(false);
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    async fn remove_container(&self, container_name: &str) -> Result<(), AppError> {
        let output = self
            .docker_output(vec![
                "container".to_string(),
                "rm".to_string(),
                "--force".to_string(),
                container_name.to_string(),
            ])
            .await?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such container") {
            return Ok(());
        }

        Err(AppError::internal(format!(
            "Failed to remove sandbox container '{container_name}': {}",
            output_message(&output)
        )))
    }

    async fn ensure_image_available(&self) -> Result<(), AppError> {
        let inspect = self
            .docker_output(vec![
                "image".to_string(),
                "inspect".to_string(),
                self.image.clone(),
            ])
            .await?;
        if inspect.status.success() {
            return Ok(());
        }

        let build = self
            .docker_output(vec![
                "build".to_string(),
                "--file".to_string(),
                self.sandbox_dockerfile.display().to_string(),
                "--tag".to_string(),
                self.image.clone(),
                ".".to_string(),
            ])
            .await?;
        if build.status.success() {
            return Ok(());
        }

        Err(AppError::internal(format!(
            "Failed to build the local sandbox image '{}': {}",
            self.image,
            output_message(&build)
        )))
    }

    async fn start_container(&self, container_name: &str) -> Result<(), AppError> {
        let output = self
            .docker_output(vec![
                "run".to_string(),
                "--detach".to_string(),
                "--rm".to_string(),
                "--name".to_string(),
                container_name.to_string(),
                "--publish".to_string(),
                format!("{}:{}", self.agent_port, DEFAULT_SANDBOX_AGENT_PORT),
                self.image.clone(),
            ])
            .await?;

        if output.status.success() {
            return Ok(());
        }

        Err(AppError::internal(format!(
            "Failed to start the local sandbox container '{}': {}",
            container_name,
            output_message(&output)
        )))
    }

    async fn wait_until_ready(&self) -> Result<(), AppError> {
        let url = format!("{}/readyz", self.agent_url());
        for _ in 0..40 {
            match self.client.get(&url).send().await {
                Ok(response) if response.status().is_success() => return Ok(()),
                _ => sleep(Duration::from_millis(250)).await,
            }
        }

        Err(AppError::internal(
            "Timed out waiting for the local sandbox agent to become ready.".to_string(),
        ))
    }

    async fn json_request<T: DeserializeOwned>(
        &self,
        builder: reqwest::RequestBuilder,
        operation: &str,
    ) -> Result<T, AppError> {
        parse_json_response(builder, operation).await
    }

    async fn bytes_request(
        &self,
        builder: reqwest::RequestBuilder,
        operation: &str,
    ) -> Result<Vec<u8>, AppError> {
        parse_bytes_response(builder, operation).await
    }
}

#[async_trait]
impl SessionRuntime for DockerSessionRuntime {
    async fn ensure_session(&self, session_identifier: &str) -> Result<(), AppError> {
        let container_name = self.container_name(session_identifier);
        if self.container_is_running(&container_name).await? {
            self.wait_until_ready().await?;
            return Ok(());
        }

        self.ensure_image_available().await?;
        self.remove_container(&container_name).await?;
        self.start_container(&container_name).await?;
        self.wait_until_ready().await
    }

    async fn status(&self, _: &str) -> Result<RuntimeStatus, AppError> {
        self.json_request(
            self.client.get(format!("{}/v1/status", self.agent_url())),
            "read sandbox status",
        )
        .await
    }

    async fn install_config(
        &self,
        _: &str,
        artifact: &ActiveConfigArtifact,
    ) -> Result<RuntimeStatus, AppError> {
        let wheel_bytes = artifact.load_wheel_bytes()?;
        self.json_request(
            self.client
                .post(format!("{}/v1/config/install", self.agent_url()))
                .header("X-ADE-Package", &artifact.package_name)
                .header("X-ADE-Version", &artifact.version)
                .header("X-ADE-Sha256", &artifact.sha256)
                .header(
                    "X-ADE-Wheel-Filename",
                    artifact
                        .wheel_path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| {
                            AppError::config(format!(
                                "Active config wheel path '{}' does not end with a valid filename.",
                                artifact.wheel_path.display()
                            ))
                        })?,
                )
                .body(wheel_bytes),
            "install the active config",
        )
        .await
    }

    async fn upload_file(
        &self,
        _: &str,
        relative_path: &str,
        content: Vec<u8>,
    ) -> Result<UploadedFile, AppError> {
        self.json_request(
            self.client
                .post(format!("{}/v1/files", self.agent_url()))
                .query(&[("path", relative_path)])
                .body(content),
            "upload a sandbox file",
        )
        .await
    }

    async fn download_file(&self, _: &str, relative_path: &str) -> Result<Vec<u8>, AppError> {
        let url = sandbox_file_url(&self.agent_url(), relative_path)?;
        self.bytes_request(self.client.get(url), "download a sandbox file")
            .await
    }

    async fn rpc(&self, _: &str, method: &str, params: Value) -> Result<Value, AppError> {
        let response: RpcResponse = self
            .json_request(
                self.client
                    .post(format!("{}/v1/rpc", self.agent_url()))
                    .json(&json!({"method": method, "params": params})),
                "call the sandbox agent",
            )
            .await?;
        Ok(response.result)
    }

    async fn poll_events(&self, _: &str, after: u64, wait_ms: u64) -> Result<EventBatch, AppError> {
        self.json_request(
            self.client
                .get(format!("{}/v1/events", self.agent_url()))
                .query(&[("after", after), ("waitMs", wait_ms)]),
            "poll sandbox events",
        )
        .await
    }

    async fn reset_session(&self, session_identifier: &str) -> Result<(), AppError> {
        self.remove_container(&self.container_name(session_identifier))
            .await
    }
}

#[derive(Clone)]
struct AzureSessionRuntime {
    api_version: String,
    audience: String,
    client: Client,
    pool_management_endpoint: String,
}

impl AzureSessionRuntime {
    fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let pool_management_endpoint =
            read_optional_trimmed_string(env, "ADE_AZURE_SESSION_POOL_ENDPOINT").ok_or_else(
                || {
                    AppError::config(
                        "Missing required environment variable: ADE_AZURE_SESSION_POOL_ENDPOINT"
                            .to_string(),
                    )
                },
            )?;

        Ok(Self {
            api_version: read_optional_trimmed_string(env, "ADE_AZURE_SESSION_API_VERSION")
                .unwrap_or_else(|| DEFAULT_AZURE_SESSION_API_VERSION.to_string()),
            audience: read_optional_trimmed_string(env, "ADE_AZURE_SESSION_AUDIENCE")
                .unwrap_or_else(|| DEFAULT_AZURE_SESSION_AUDIENCE.to_string()),
            client: Client::new(),
            pool_management_endpoint,
        })
    }

    fn session_url(
        &self,
        session_identifier: &str,
        path: &str,
        extra_query: &[(&str, String)],
    ) -> Result<Url, AppError> {
        let mut url = Url::parse(&format!(
            "{}/{}",
            self.pool_management_endpoint.trim_end_matches('/'),
            path.trim_start_matches('/'),
        ))
        .map_err(|error| {
            AppError::config_with_source(
                "ADE_AZURE_SESSION_POOL_ENDPOINT is not a valid URL.".to_string(),
                error,
            )
        })?;

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("identifier", session_identifier);
            query.append_pair("api-version", &self.api_version);
            for (key, value) in extra_query {
                query.append_pair(key, value);
            }
        }

        Ok(url)
    }

    async fn bearer_token(&self) -> Result<String, AppError> {
        let scope = format!("{}/.default", self.audience.trim_end_matches('/'));

        if let Ok(credential) = ManagedIdentityCredential::new(None) {
            if let Ok(token) = credential.get_token(&[scope.as_str()], None).await {
                return Ok(token.token.secret().to_string());
            }
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
                    "Failed to acquire an Azure access token for session runtime calls."
                        .to_string(),
                    error,
                )
            })?;
        Ok(token.token.secret().to_string())
    }

    async fn json_request<T: DeserializeOwned>(
        &self,
        method: Method,
        session_identifier: &str,
        path: &str,
        extra_query: &[(&str, String)],
        body: Option<Vec<u8>>,
        headers: &[(&str, String)],
        operation: &str,
    ) -> Result<T, AppError> {
        let token = self.bearer_token().await?;
        let url = self.session_url(session_identifier, path, extra_query)?;
        let mut request = self.client.request(method, url).bearer_auth(token);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        parse_json_response(request, operation).await
    }

    async fn bytes_request(
        &self,
        builder: reqwest::RequestBuilder,
        operation: &str,
    ) -> Result<Vec<u8>, AppError> {
        parse_bytes_response(builder, operation).await
    }
}

#[async_trait]
impl SessionRuntime for AzureSessionRuntime {
    async fn ensure_session(&self, session_identifier: &str) -> Result<(), AppError> {
        for _ in 0..40 {
            let result: Result<HealthStatus, AppError> = self
                .json_request(
                    Method::GET,
                    session_identifier,
                    "/readyz",
                    &[],
                    None,
                    &[],
                    "ensure the Azure sandbox session",
                )
                .await;
            if let Ok(status) = result {
                if status.status == "ready" {
                    return Ok(());
                }
            }
            sleep(Duration::from_millis(250)).await;
        }

        Err(AppError::internal(
            "Timed out waiting for the Azure sandbox session to become ready.".to_string(),
        ))
    }

    async fn status(&self, session_identifier: &str) -> Result<RuntimeStatus, AppError> {
        self.json_request(
            Method::GET,
            session_identifier,
            "/v1/status",
            &[],
            None,
            &[],
            "read sandbox status",
        )
        .await
    }

    async fn install_config(
        &self,
        session_identifier: &str,
        artifact: &ActiveConfigArtifact,
    ) -> Result<RuntimeStatus, AppError> {
        let wheel_bytes = artifact.load_wheel_bytes()?;
        self.json_request(
            Method::POST,
            session_identifier,
            "/v1/config/install",
            &[],
            Some(wheel_bytes),
            &[
                ("X-ADE-Package", artifact.package_name.clone()),
                ("X-ADE-Version", artifact.version.clone()),
                ("X-ADE-Sha256", artifact.sha256.clone()),
                (
                    "X-ADE-Wheel-Filename",
                    artifact
                        .wheel_path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| {
                            AppError::config(format!(
                                "Active config wheel path '{}' does not end with a valid filename.",
                                artifact.wheel_path.display()
                            ))
                        })?
                        .to_string(),
                ),
            ],
            "install the active config",
        )
        .await
    }

    async fn upload_file(
        &self,
        session_identifier: &str,
        relative_path: &str,
        content: Vec<u8>,
    ) -> Result<UploadedFile, AppError> {
        self.json_request(
            Method::POST,
            session_identifier,
            "/v1/files",
            &[("path", relative_path.to_string())],
            Some(content),
            &[],
            "upload a sandbox file",
        )
        .await
    }

    async fn download_file(
        &self,
        session_identifier: &str,
        relative_path: &str,
    ) -> Result<Vec<u8>, AppError> {
        let mut url = self.session_url(session_identifier, "/v1/files", &[])?;
        append_path_segments(&mut url, relative_path)?;
        self.bytes_request(
            self.client.get(url).bearer_auth(self.bearer_token().await?),
            "download a sandbox file",
        )
        .await
    }

    async fn rpc(
        &self,
        session_identifier: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, AppError> {
        let token = self.bearer_token().await?;
        let url = self.session_url(session_identifier, "/v1/rpc", &[])?;
        let response: RpcResponse = parse_json_response(
            self.client
                .post(url)
                .bearer_auth(token)
                .json(&json!({"method": method, "params": params})),
            "call the sandbox agent",
        )
        .await?;
        Ok(response.result)
    }

    async fn poll_events(
        &self,
        session_identifier: &str,
        after: u64,
        wait_ms: u64,
    ) -> Result<EventBatch, AppError> {
        self.json_request(
            Method::GET,
            session_identifier,
            "/v1/events",
            &[
                ("after", after.to_string()),
                ("waitMs", wait_ms.to_string()),
            ],
            None,
            &[],
            "poll sandbox events",
        )
        .await
    }

    async fn reset_session(&self, session_identifier: &str) -> Result<(), AppError> {
        let token = self.bearer_token().await?;
        let url = self.session_url(session_identifier, "/.management/stopSession", &[])?;
        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| {
                AppError::internal_with_source(
                    "Failed to stop the Azure sandbox session.".to_string(),
                    error,
                )
            })?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let message = error_message(response).await?;
        Err(map_runtime_http_error(
            status,
            format!("Failed to stop the Azure sandbox session: {message}"),
        ))
    }
}

#[derive(Deserialize)]
struct RpcResponse {
    result: Value,
}

#[derive(Deserialize)]
struct HealthStatus {
    status: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

async fn parse_json_response<T: DeserializeOwned>(
    builder: reqwest::RequestBuilder,
    operation: &str,
) -> Result<T, AppError> {
    let response = builder.send().await.map_err(|error| {
        AppError::internal_with_source(format!("Failed to {operation}."), error)
    })?;
    let status = response.status();

    if !status.is_success() {
        let message = error_message(response).await?;
        return Err(map_runtime_http_error(status, message));
    }

    response.json().await.map_err(|error| {
        AppError::internal_with_source(
            format!("Failed to decode the runtime response while trying to {operation}."),
            error,
        )
    })
}

async fn parse_bytes_response(
    builder: reqwest::RequestBuilder,
    operation: &str,
) -> Result<Vec<u8>, AppError> {
    let response = builder.send().await.map_err(|error| {
        AppError::internal_with_source(format!("Failed to {operation}."), error)
    })?;
    let status = response.status();

    if !status.is_success() {
        let message = error_message(response).await?;
        return Err(map_runtime_http_error(status, message));
    }

    response
        .bytes()
        .await
        .map(|value| value.to_vec())
        .map_err(|error| {
            AppError::internal_with_source(
                format!("Failed to read the runtime response while trying to {operation}."),
                error,
            )
        })
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
    if let Ok(body) = serde_json::from_slice::<ErrorBody>(&bytes) {
        if let Some(message) = body.message {
            return Ok(message);
        }
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

fn sandbox_file_url(base_url: &str, relative_path: &str) -> Result<Url, AppError> {
    let mut url = Url::parse(base_url).map_err(|error| {
        AppError::internal_with_source(format!("Invalid sandbox agent URL: '{base_url}'."), error)
    })?;
    append_path_segments(&mut url, "v1/files")?;
    append_path_segments(&mut url, relative_path)?;
    Ok(url)
}

fn append_path_segments(url: &mut Url, relative_path: &str) -> Result<(), AppError> {
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| AppError::internal("Failed to build the sandbox file URL.".to_string()))?;
    for segment in relative_path
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        segments.push(segment);
    }
    drop(segments);
    Ok(())
}

fn hex_sha256(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

fn output_message(output: &Output) -> String {
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
    if service.is_some() {
        info!("Hosted ADE runtime is enabled.");
    } else {
        info!("Hosted ADE runtime is disabled.");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use axum::response::IntoResponse;
    use reqwest::Client;
    use tempfile::tempdir;

    use super::{
        AzureSessionRuntime, DockerSessionRuntime, append_path_segments, derive_session_identifier,
        map_runtime_http_error, sandbox_file_url,
    };

    #[test]
    fn derives_stable_session_identifiers() {
        let first = derive_session_identifier("secret", "ade-config@0.1.0:abc");
        let second = derive_session_identifier("secret", "ade-config@0.1.0:abc");

        assert_eq!(first, second);
        assert!(first.starts_with("cfg-"));
    }

    #[test]
    fn sandbox_file_urls_encode_each_path_segment() {
        let url =
            sandbox_file_url("http://127.0.0.1:9100", "inputs/folder one/report#1.csv").unwrap();

        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:9100/v1/files/inputs/folder%20one/report%231.csv"
        );
    }

    #[test]
    fn azure_file_urls_encode_each_path_segment() {
        let runtime = AzureSessionRuntime {
            api_version: "2025-10-02-preview".to_string(),
            audience: "https://dynamicsessions.io".to_string(),
            client: Client::new(),
            pool_management_endpoint: "https://example.test".to_string(),
        };
        let mut url = runtime.session_url("cfg-test", "/v1/files", &[]).unwrap();
        append_path_segments(&mut url, "outputs/final report#.xlsx").unwrap();

        assert_eq!(
            url.as_str(),
            "https://example.test/v1/files/outputs/final%20report%23.xlsx?identifier=cfg-test&api-version=2025-10-02-preview"
        );
    }

    #[tokio::test]
    async fn docker_output_runs_from_the_explicit_repo_root() {
        let repo_root = tempdir().unwrap();
        let runtime = DockerSessionRuntime {
            client: Client::new(),
            docker_command: "/bin/pwd".to_string(),
            image: "ignored".to_string(),
            container_name_prefix: "ignored".to_string(),
            agent_host: "127.0.0.1".to_string(),
            agent_port: 9100,
            repo_root: repo_root.path().to_path_buf(),
            sandbox_dockerfile: PathBuf::from("Dockerfile.sandbox"),
        };

        let output = runtime.docker_output(vec![]).await.unwrap();
        let actual = std::fs::canonicalize(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
        let expected = std::fs::canonicalize(repo_root.path()).unwrap();

        assert!(output.status.success());
        assert_eq!(actual, expected);
    }

    #[test]
    fn runtime_http_errors_preserve_upstream_status_codes() {
        let response = map_runtime_http_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Too many requests".to_string(),
        )
        .into_response();

        assert_eq!(response.status().as_u16(), 429);
    }
}
