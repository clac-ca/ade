mod client;
mod python;

use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use utoipa::{IntoParams, ToSchema};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
};

use self::{
    client::SessionPoolClient,
    python::{
        RunPythonConfig, build_run_code, command_execution_code, ensure_successful_execution,
        extract_command_response, extract_run_response,
    },
};

pub(crate) use self::client::SessionFile;

const CONFIG_PACKAGE_NAME: &str = "ade-config";
const CONFIG_TARGETS_ENV_NAME: &str = "ADE_CONFIG_TARGETS";
const ENGINE_PACKAGE_NAME: &str = "ade-engine";
const ENGINE_WHEEL_ENV_NAME: &str = "ADE_ENGINE_WHEEL_PATH";
const INSTALL_LOCK_SESSION_FILENAME: &str = ".ade-session-install.lock";
const RUNS_ROOT: &str = "runs";
const SESSION_ROOT: &str = "/mnt/data";
const SESSION_SECRET_ENV_NAME: &str = "ADE_SESSION_SECRET";

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, ToSchema, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct Scope {
    /// Workspace id.
    #[serde(rename = "workspaceId")]
    pub(crate) workspace_id: String,
    /// Config version id.
    #[serde(rename = "configVersionId")]
    pub(crate) config_version_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExecuteCommandRequest {
    pub(crate) shell_command: String,
    pub(crate) timeout_in_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExecuteCommandResponse {
    pub(crate) duration_ms: u64,
    pub(crate) exit_code: i64,
    pub(crate) stderr: String,
    pub(crate) stdout: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateRunRequest {
    pub(crate) input_path: String,
    pub(crate) timeout_in_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResponse {
    pub(crate) output_path: String,
    pub(crate) validation_issues: Vec<RunValidationIssue>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunValidationIssue {
    pub(crate) row_index: usize,
    pub(crate) field: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PackageWheel {
    bytes: Vec<u8>,
    filename: String,
    version: String,
}

pub struct SessionService {
    client: SessionPoolClient,
    config_targets: HashMap<Scope, PackageWheel>,
    engine: PackageWheel,
    session_secret: String,
}

impl SessionService {
    pub fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let session_secret = read_optional_trimmed_string(env, SESSION_SECRET_ENV_NAME)
            .ok_or_else(|| {
                AppError::config(format!(
                    "Missing required environment variable: {SESSION_SECRET_ENV_NAME}"
                ))
            })?;

        Ok(Self {
            client: SessionPoolClient::from_env(env)?,
            config_targets: resolve_config_targets(env)?,
            engine: resolve_required_wheel(env, ENGINE_WHEEL_ENV_NAME, "ade_engine")?,
            session_secret,
        })
    }

    pub(crate) async fn execute_command(
        &self,
        scope: &Scope,
        shell_command: &str,
        timeout_in_seconds: Option<u64>,
    ) -> Result<ExecuteCommandResponse, AppError> {
        let execution = self
            .execute_python(
                &self.session_identifier(scope),
                command_execution_code(shell_command)?,
                timeout_in_seconds,
            )
            .await?;
        extract_command_response(execution)
    }

    pub(crate) async fn upload_file(
        &self,
        scope: &Scope,
        filename: String,
        content_type: Option<String>,
        content: Vec<u8>,
    ) -> Result<SessionFile, AppError> {
        self.client
            .upload_file(
                &self.session_identifier(scope),
                uploaded_filename(&filename)?,
                content_type,
                content,
            )
            .await
    }

    pub(crate) async fn list_files(&self, scope: &Scope) -> Result<Vec<SessionFile>, AppError> {
        let config = self.config_for(scope)?;
        let mut files = self
            .client
            .list_files(&self.session_identifier(scope))
            .await?
            .into_iter()
            .filter(|file| !is_internal_session_file(&self.engine, config, &file.filename))
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.filename.cmp(&right.filename));
        Ok(files)
    }

    pub(crate) async fn download_file(
        &self,
        scope: &Scope,
        filename: &str,
    ) -> Result<(String, Vec<u8>), AppError> {
        let config = self.config_for(scope)?;
        let normalized_path = public_session_path(filename)?;
        if is_internal_session_file(&self.engine, config, &normalized_path) {
            return Err(AppError::not_found("Session file not found."));
        }
        self.client
            .download_file(&self.session_identifier(scope), &normalized_path)
            .await
    }

    pub(crate) async fn run(
        &self,
        scope: &Scope,
        input_path: &str,
        timeout_in_seconds: Option<u64>,
    ) -> Result<RunResponse, AppError> {
        let config = self.config_for(scope)?;
        let session_identifier = self.session_identifier(scope);
        let normalized_input_path = public_session_path(input_path)?;

        if is_internal_session_file(&self.engine, config, &normalized_input_path) {
            return Err(AppError::not_found("Session file not found."));
        }

        self.client
            .upload_file(
                &session_identifier,
                self.engine.filename.clone(),
                Some("application/octet-stream".to_string()),
                self.engine.bytes.clone(),
            )
            .await?;
        self.client
            .upload_file(
                &session_identifier,
                config.filename.clone(),
                Some("application/octet-stream".to_string()),
                config.bytes.clone(),
            )
            .await?;

        let execution = self
            .execute_python(
                &session_identifier,
                build_run_code(&RunPythonConfig {
                    config_package_name: CONFIG_PACKAGE_NAME,
                    config_version: config.version.clone(),
                    config_wheel_path: session_file_path(&config.filename),
                    engine_package_name: ENGINE_PACKAGE_NAME,
                    engine_version: self.engine.version.clone(),
                    engine_wheel_path: session_file_path(&self.engine.filename),
                    input_path: session_file_path(&normalized_input_path),
                    install_lock_path: session_file_path(INSTALL_LOCK_SESSION_FILENAME),
                    runs_root: session_file_path(RUNS_ROOT),
                    sentinel_prefix: python::RUN_SENTINEL_PREFIX,
                })?,
                timeout_in_seconds,
            )
            .await?;

        ensure_successful_execution(&execution)?;
        extract_run_response(&execution)
    }

    async fn execute_python(
        &self,
        session_identifier: &str,
        code: String,
        timeout_in_seconds: Option<u64>,
    ) -> Result<client::PythonExecution, AppError> {
        self.client
            .execute(session_identifier, code, timeout_in_seconds)
            .await
    }

    fn config_for(&self, scope: &Scope) -> Result<&PackageWheel, AppError> {
        self.config_targets.get(scope).ok_or_else(|| {
            AppError::not_found(format!(
                "Config version '{}' for workspace '{}' is not configured.",
                scope.config_version_id, scope.workspace_id
            ))
        })
    }

    fn session_identifier(&self, scope: &Scope) -> String {
        derive_session_identifier(
            &self.session_secret,
            &format!("{}:{}", scope.workspace_id, scope.config_version_id),
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigTargetEntry {
    workspace_id: String,
    config_version_id: String,
    wheel_path: PathBuf,
}

fn uploaded_filename(filename: &str) -> Result<String, AppError> {
    let name = Path::new(filename.trim())
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::request("Uploaded file must include a valid filename."))?;

    if name.is_empty() {
        return Err(AppError::request(
            "Uploaded file must include a valid filename.",
        ));
    }

    Ok(name.to_string())
}

fn public_session_path(path: &str) -> Result<String, AppError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::request(
            "Session file path must be a non-empty relative path.",
        ));
    }

    let mut segments = Vec::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => segments.push(segment.to_string_lossy().to_string()),
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
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

    let normalized = segments.join("/");
    if normalized == INSTALL_LOCK_SESSION_FILENAME {
        return Err(AppError::not_found("Session file not found."));
    }

    Ok(normalized)
}

fn session_file_path(relative_path: &str) -> String {
    format!("{SESSION_ROOT}/{}", relative_path.trim_start_matches('/'))
}

fn is_internal_session_file(engine: &PackageWheel, config: &PackageWheel, path: &str) -> bool {
    path == engine.filename || path == config.filename || path == INSTALL_LOCK_SESSION_FILENAME
}

fn resolve_config_targets(env: &EnvBag) -> Result<HashMap<Scope, PackageWheel>, AppError> {
    let targets_json =
        read_optional_trimmed_string(env, CONFIG_TARGETS_ENV_NAME).ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {CONFIG_TARGETS_ENV_NAME}"
            ))
        })?;
    let targets =
        serde_json::from_str::<Vec<ConfigTargetEntry>>(&targets_json).map_err(|error| {
            AppError::config_with_source(
                format!("Environment variable {CONFIG_TARGETS_ENV_NAME} must be valid JSON."),
                error,
            )
        })?;

    if targets.is_empty() {
        return Err(AppError::config(format!(
            "Environment variable {CONFIG_TARGETS_ENV_NAME} must include at least one config target."
        )));
    }

    let mut resolved = HashMap::new();
    for target in targets {
        let scope = Scope {
            workspace_id: target.workspace_id,
            config_version_id: target.config_version_id,
        };
        let wheel =
            resolve_wheel_from_path(CONFIG_TARGETS_ENV_NAME, "ade_config", &target.wheel_path)?;
        if resolved.insert(scope.clone(), wheel).is_some() {
            return Err(AppError::config(format!(
                "Duplicate config target '{}:{}' in {CONFIG_TARGETS_ENV_NAME}.",
                scope.workspace_id, scope.config_version_id
            )));
        }
    }

    Ok(resolved)
}

fn resolve_required_wheel(
    env: &EnvBag,
    wheel_path_env_name: &str,
    wheel_prefix: &str,
) -> Result<PackageWheel, AppError> {
    let wheel_path = PathBuf::from(
        read_optional_trimmed_string(env, wheel_path_env_name).ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {wheel_path_env_name}"
            ))
        })?,
    );
    resolve_wheel_from_path(wheel_path_env_name, wheel_prefix, &wheel_path)
}

fn resolve_wheel_from_path(
    wheel_path_env_name: &str,
    wheel_prefix: &str,
    wheel_path: &Path,
) -> Result<PackageWheel, AppError> {
    if !wheel_path.is_file() {
        return Err(AppError::config(format!(
            "Python package wheel configured by {wheel_path_env_name} was not found at '{}'.",
            wheel_path.display()
        )));
    }

    let resolved_wheel_path = fs::canonicalize(wheel_path).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to resolve the Python package wheel from '{}'.",
                wheel_path.display()
            ),
            error,
        )
    })?;
    let filename = wheel_filename(&resolved_wheel_path)?;
    let version = parse_wheel_version(&filename, wheel_prefix).ok_or_else(|| {
        AppError::config(format!(
            "Unable to determine the package version from '{}'.",
            wheel_path.display()
        ))
    })?;
    let bytes = fs::read(wheel_path).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to read the Python package wheel from '{}'.",
                wheel_path.display()
            ),
            error,
        )
    })?;

    Ok(PackageWheel {
        bytes,
        filename,
        version,
    })
}

fn wheel_filename(wheel_path: &Path) -> Result<String, AppError> {
    wheel_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            AppError::config(format!(
                "Python package wheel path '{}' does not end with a valid filename.",
                wheel_path.display()
            ))
        })
}

fn parse_wheel_version(filename: &str, wheel_prefix: &str) -> Option<String> {
    let prefix = format!("{wheel_prefix}-");
    let remainder = filename.strip_prefix(&prefix)?;
    let without_extension = remainder.strip_suffix(".whl")?;
    without_extension.split('-').next().map(ToOwned::to_owned)
}

fn derive_session_identifier(secret: &str, scope: &str) -> String {
    type HmacSha256 = Hmac<Sha256>;

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts arbitrary key lengths");
    mac.update(scope.as_bytes());
    let digest = hex::encode(mac.finalize().into_bytes());
    format!("cfg-{}", &digest[..32])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CONFIG_PACKAGE_NAME, CreateRunRequest, ENGINE_PACKAGE_NAME, ExecuteCommandRequest,
        ExecuteCommandResponse, RunResponse, RunValidationIssue, Scope, build_run_code,
        client::PythonExecution,
        command_execution_code, derive_session_identifier, ensure_successful_execution,
        extract_command_response, extract_run_response, parse_wheel_version, public_session_path,
        python::{
            COMMAND_SENTINEL_PREFIX, RUN_SENTINEL_PREFIX, RunPythonConfig, strip_command_metadata,
        },
    };

    #[test]
    fn parses_wheel_versions_from_standard_filenames() {
        assert_eq!(
            parse_wheel_version("ade_engine-0.1.0-py3-none-any.whl", "ade_engine"),
            Some("0.1.0".to_string())
        );
        assert_eq!(
            parse_wheel_version("ade_config-2026.3.1-py3-none-any.whl", "ade_config"),
            Some("2026.3.1".to_string())
        );
    }

    #[test]
    fn session_identifiers_are_stable_and_scope_specific() {
        let first = derive_session_identifier("secret", "workspace-a:config-v1");
        let second = derive_session_identifier("secret", "workspace-a:config-v1");
        let third = derive_session_identifier("secret", "workspace-b:config-v1");

        assert_eq!(first, second);
        assert_ne!(first, third);
        assert!(first.starts_with("cfg-"));
    }

    #[test]
    fn command_request_denies_unknown_fields() {
        let error = serde_json::from_value::<ExecuteCommandRequest>(json!({
            "code": "print('hi')",
        }))
        .unwrap_err()
        .to_string();

        assert!(error.contains("unknown field"));
    }

    #[test]
    fn run_request_denies_unknown_fields() {
        let error = serde_json::from_value::<CreateRunRequest>(json!({
            "file": "input.csv",
        }))
        .unwrap_err()
        .to_string();

        assert!(error.contains("unknown field"));
    }

    #[test]
    fn shell_command_code_uses_json_config_and_subprocess() {
        let code = command_execution_code("pwd").unwrap();

        assert!(code.contains("json.loads"));
        assert!(code.contains("subprocess.run"));
        assert!(code.contains("CONFIG[\"command\"]"));
        assert!(code.contains("cwd=\"/mnt/data\""));
    }

    #[test]
    fn command_metadata_is_extracted_into_a_flat_response() {
        let response = extract_command_response(PythonExecution {
            duration_ms: 4,
            status: "Succeeded".to_string(),
            stdout: format!("hello\n{COMMAND_SENTINEL_PREFIX}{{\"exitCode\":7}}\n"),
            stderr: String::new(),
        })
        .unwrap();

        assert_eq!(
            response,
            ExecuteCommandResponse {
                duration_ms: 4,
                exit_code: 7,
                stderr: String::new(),
                stdout: "hello".to_string(),
            }
        );
    }

    #[test]
    fn strip_command_metadata_returns_none_when_missing() {
        assert_eq!(strip_command_metadata("plain output").unwrap(), None);
    }

    #[test]
    fn extracts_run_responses_from_stdout_metadata() {
        let response = PythonExecution {
            duration_ms: 0,
            status: "Succeeded".to_string(),
            stdout: format!(
                "log line\n{RUN_SENTINEL_PREFIX}{{\"outputPath\":\"runs/run-1/output/input.normalized.xlsx\",\"validationIssues\":[{{\"rowIndex\":2,\"field\":\"email\",\"message\":\"missing\"}}]}}\n"
            ),
            stderr: String::new(),
        };

        assert_eq!(
            extract_run_response(&response).unwrap(),
            RunResponse {
                output_path: "runs/run-1/output/input.normalized.xlsx".to_string(),
                validation_issues: vec![RunValidationIssue {
                    row_index: 2,
                    field: "email".to_string(),
                    message: "missing".to_string(),
                }],
            }
        );
    }

    #[test]
    fn successful_status_is_accepted() {
        ensure_successful_execution(&PythonExecution {
            duration_ms: 0,
            status: "Succeeded".to_string(),
            stdout: "ok".to_string(),
            stderr: String::new(),
        })
        .unwrap();
    }

    #[test]
    fn scope_deserializes_from_workspace_and_config_paths() {
        let scope = serde_json::from_value::<Scope>(json!({
            "workspaceId": "workspace-a",
            "configVersionId": "config-v1",
        }))
        .unwrap();

        assert_eq!(scope.workspace_id, "workspace-a");
        assert_eq!(scope.config_version_id, "config-v1");
    }

    #[test]
    fn public_session_paths_must_be_relative_and_not_internal() {
        assert_eq!(public_session_path("input.csv").unwrap(), "input.csv");
        assert_eq!(
            public_session_path("runs/run-1/output/input.xlsx").unwrap(),
            "runs/run-1/output/input.xlsx"
        );
        assert!(public_session_path("/input.csv").is_err());
        assert!(public_session_path("../input.csv").is_err());
        assert!(public_session_path(".ade-session-install.lock").is_err());
    }

    #[test]
    fn run_template_uses_a_package_install_lock_and_runs_root() {
        let code = build_run_code(&RunPythonConfig {
            config_package_name: CONFIG_PACKAGE_NAME,
            config_version: "2.0.0".to_string(),
            config_wheel_path: "/mnt/data/ade_config-2.0.0-py3-none-any.whl".to_string(),
            engine_package_name: ENGINE_PACKAGE_NAME,
            engine_version: "1.0.0".to_string(),
            engine_wheel_path: "/mnt/data/ade_engine-1.0.0-py3-none-any.whl".to_string(),
            input_path: "/mnt/data/input.csv".to_string(),
            install_lock_path: "/mnt/data/.ade-session-install.lock".to_string(),
            runs_root: "/mnt/data/runs".to_string(),
            sentinel_prefix: RUN_SENTINEL_PREFIX,
        })
        .unwrap();

        assert!(code.contains("json.loads"));
        assert!(code.contains("fcntl.flock"));
        assert!(code.contains("tempfile.mkdtemp"));
        assert!(code.contains("/mnt/data/runs"));
    }
}
