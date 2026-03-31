use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
};

use super::client::{SessionExecution, SessionFile, SessionOperationResult, SessionPoolClient};

const AGENT_BINARY_ENV_NAME: &str = "ADE_SESSION_AGENT_BINARY_PATH";
const AGENT_BINARY_SESSION_PATH: &str = ".ade/bin/ade-session-agent";
const AGENT_CONFIG_SESSION_PATH: &str = ".ade/session-agent.json";
const CONFIG_PACKAGE_NAME: &str = "ade-config";
const CONFIG_TARGETS_ENV_NAME: &str = "ADE_CONFIG_TARGETS";
const DEFAULT_AGENT_BINARY_BASENAME: &str = "ade-session-agent";
const DEFAULT_TOOLCHAIN_BUNDLE_PATH: &str = "/app/python/python-3.14.0-linux-x86_64.tar.gz";
const ENGINE_PACKAGE_NAME: &str = "ade-engine";
const ENGINE_WHEEL_ENV_NAME: &str = "ADE_ENGINE_WHEEL_PATH";
const PYTHON_HOME_ROOT: &str = "/mnt/data/.ade/runtime/python";
const PYTHON_TOOLCHAIN_BUNDLE_ENV_NAME: &str = "ADE_PYTHON_TOOLCHAIN_BUNDLE_PATH";
const PYTHON_TOOLCHAIN_SESSION_DIR: &str = ".ade/toolchains";
const SESSION_ROOT: &str = "/mnt/data";
const SESSION_SECRET_ENV_NAME: &str = "ADE_SESSION_SECRET";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ScopeSessionId(String);

impl ScopeSessionId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileArtifact {
    bytes: Vec<u8>,
    filename: String,
    version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRuntimeArtifacts {
    pub(crate) agent_binary_bytes: Vec<u8>,
    pub(crate) agent_binary_filename: String,
    pub(crate) agent_binary_path: String,
    pub(crate) agent_launch_config_filename: String,
    pub(crate) agent_launch_config_path: String,
    pub(crate) config_filename: String,
    pub(crate) config_package_name: &'static str,
    pub(crate) config_version: String,
    pub(crate) config_wheel_bytes: Vec<u8>,
    pub(crate) config_wheel_path: String,
    pub(crate) engine_filename: String,
    pub(crate) engine_package_name: &'static str,
    pub(crate) engine_version: String,
    pub(crate) engine_wheel_bytes: Vec<u8>,
    pub(crate) engine_wheel_path: String,
    pub(crate) python_executable_path: String,
    pub(crate) python_home_path: String,
    pub(crate) python_toolchain_bytes: Vec<u8>,
    pub(crate) python_toolchain_filename: String,
    pub(crate) python_toolchain_path: String,
    pub(crate) python_toolchain_version: String,
    pub(crate) session_root: String,
}

pub struct SessionService {
    agent_binary: FileArtifact,
    client: SessionPoolClient,
    config_targets: HashMap<Scope, FileArtifact>,
    engine: FileArtifact,
    python_toolchain: FileArtifact,
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
            agent_binary: resolve_agent_binary(env)?,
            client: SessionPoolClient::from_env(env)?,
            config_targets: resolve_config_targets(env)?,
            engine: resolve_required_file(env, ENGINE_WHEEL_ENV_NAME, "ade_engine", ".whl")?,
            python_toolchain: resolve_python_toolchain(env)?,
            session_secret,
        })
    }

    pub(crate) fn scope_session_id(&self, scope: &Scope) -> ScopeSessionId {
        ScopeSessionId(derive_session_identifier(
            &self.session_secret,
            &format!("{}:{}", scope.workspace_id, scope.config_version_id),
        ))
    }

    pub(crate) fn session_secret(&self) -> &str {
        &self.session_secret
    }

    pub(crate) fn runtime_artifacts(
        &self,
        scope: &Scope,
    ) -> Result<SessionRuntimeArtifacts, AppError> {
        let config = self.config_targets.get(scope).ok_or_else(|| {
            AppError::not_found(format!(
                "Config version '{}' for workspace '{}' is not configured.",
                scope.config_version_id, scope.workspace_id
            ))
        })?;
        let python_home_path = format!("{PYTHON_HOME_ROOT}/{}", self.python_toolchain.version);

        Ok(SessionRuntimeArtifacts {
            agent_binary_bytes: self.agent_binary.bytes.clone(),
            agent_binary_filename: AGENT_BINARY_SESSION_PATH.to_string(),
            agent_binary_path: session_absolute_path(AGENT_BINARY_SESSION_PATH),
            agent_launch_config_filename: AGENT_CONFIG_SESSION_PATH.to_string(),
            agent_launch_config_path: session_absolute_path(AGENT_CONFIG_SESSION_PATH),
            config_filename: config.filename.clone(),
            config_package_name: CONFIG_PACKAGE_NAME,
            config_version: config.version.clone(),
            config_wheel_bytes: config.bytes.clone(),
            config_wheel_path: session_absolute_path(&config.filename),
            engine_filename: self.engine.filename.clone(),
            engine_package_name: ENGINE_PACKAGE_NAME,
            engine_version: self.engine.version.clone(),
            engine_wheel_bytes: self.engine.bytes.clone(),
            engine_wheel_path: session_absolute_path(&self.engine.filename),
            python_executable_path: format!("{python_home_path}/bin/python3"),
            python_home_path: python_home_path.clone(),
            python_toolchain_bytes: self.python_toolchain.bytes.clone(),
            python_toolchain_filename: format!(
                "{PYTHON_TOOLCHAIN_SESSION_DIR}/{}",
                self.python_toolchain.filename
            ),
            python_toolchain_path: session_absolute_path(&format!(
                "{PYTHON_TOOLCHAIN_SESSION_DIR}/{}",
                self.python_toolchain.filename
            )),
            python_toolchain_version: self.python_toolchain.version.clone(),
            session_root: SESSION_ROOT.to_string(),
        })
    }

    pub(crate) async fn upload_session_file(
        &self,
        scope: &Scope,
        path: &str,
        content_type: Option<String>,
        content: Vec<u8>,
    ) -> Result<SessionOperationResult<SessionFile>, AppError> {
        self.client
            .upload_file(
                &self.scope_session_id(scope).0,
                public_session_path(path)?,
                content_type,
                content,
            )
            .await
    }

    pub(crate) async fn execute_shell_detailed(
        &self,
        scope: &Scope,
        shell_command: String,
        timeout_in_seconds: Option<u64>,
    ) -> Result<SessionOperationResult<SessionExecution>, AppError> {
        self.client
            .execute(
                self.scope_session_id(scope).as_str(),
                shell_command,
                timeout_in_seconds,
            )
            .await
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigTargetEntry {
    workspace_id: String,
    config_version_id: String,
    wheel_path: PathBuf,
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
    Ok(normalized)
}

fn session_absolute_path(path: &str) -> String {
    format!("{SESSION_ROOT}/{}", path.trim_start_matches('/'))
}

fn resolve_config_targets(env: &EnvBag) -> Result<HashMap<Scope, FileArtifact>, AppError> {
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
        let wheel = resolve_file_from_path(
            CONFIG_TARGETS_ENV_NAME,
            "ade_config",
            ".whl",
            &target.wheel_path,
        )?;
        if resolved.insert(scope.clone(), wheel).is_some() {
            return Err(AppError::config(format!(
                "Duplicate config target '{}:{}' in {CONFIG_TARGETS_ENV_NAME}.",
                scope.workspace_id, scope.config_version_id
            )));
        }
    }

    Ok(resolved)
}

fn resolve_required_file(
    env: &EnvBag,
    env_name: &str,
    prefix: &str,
    required_extension: &str,
) -> Result<FileArtifact, AppError> {
    let path = PathBuf::from(read_optional_trimmed_string(env, env_name).ok_or_else(|| {
        AppError::config(format!("Missing required environment variable: {env_name}"))
    })?);
    resolve_file_from_path(env_name, prefix, required_extension, &path)
}

fn resolve_file_from_path(
    env_name: &str,
    prefix: &str,
    required_extension: &str,
    path: &Path,
) -> Result<FileArtifact, AppError> {
    if !path.is_file() {
        return Err(AppError::config(format!(
            "Runtime artifact configured by {env_name} was not found at '{}'.",
            path.display()
        )));
    }

    let resolved_path = fs::canonicalize(path).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to resolve the runtime artifact from '{}'.",
                path.display()
            ),
            error,
        )
    })?;
    let filename = wheel_filename(&resolved_path)?;
    if !filename.ends_with(required_extension) {
        return Err(AppError::config(format!(
            "Runtime artifact '{}' must end with '{required_extension}'.",
            path.display()
        )));
    }
    let version =
        parse_artifact_version(&filename, prefix, required_extension).ok_or_else(|| {
            AppError::config(format!(
                "Unable to determine the runtime artifact version from '{}'.",
                path.display()
            ))
        })?;
    let bytes = fs::read(path).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to read the runtime artifact from '{}'.",
                path.display()
            ),
            error,
        )
    })?;

    Ok(FileArtifact {
        bytes,
        filename,
        version,
    })
}

fn resolve_agent_binary(env: &EnvBag) -> Result<FileArtifact, AppError> {
    let configured = read_optional_trimmed_string(env, AGENT_BINARY_ENV_NAME)
        .map(PathBuf::from)
        .unwrap_or_else(default_agent_binary_path);
    resolve_file_from_path(
        AGENT_BINARY_ENV_NAME,
        DEFAULT_AGENT_BINARY_BASENAME,
        if cfg!(windows) { ".exe" } else { "" },
        &configured,
    )
}

fn default_agent_binary_path() -> PathBuf {
    let mut path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    path.set_file_name(if cfg!(windows) {
        "ade-session-agent.exe"
    } else {
        "ade-session-agent"
    });
    path
}

fn resolve_python_toolchain(env: &EnvBag) -> Result<FileArtifact, AppError> {
    let configured = read_optional_trimmed_string(env, PYTHON_TOOLCHAIN_BUNDLE_ENV_NAME)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_TOOLCHAIN_BUNDLE_PATH));
    resolve_file_from_path(
        PYTHON_TOOLCHAIN_BUNDLE_ENV_NAME,
        "python",
        ".tar.gz",
        &configured,
    )
}

fn wheel_filename(wheel_path: &Path) -> Result<String, AppError> {
    wheel_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            AppError::config(format!(
                "Runtime artifact path '{}' does not end with a valid filename.",
                wheel_path.display()
            ))
        })
}

fn parse_artifact_version(filename: &str, prefix: &str, extension: &str) -> Option<String> {
    if extension.is_empty() {
        if filename == prefix {
            return Some("dev".to_string());
        }
        return filename
            .strip_prefix(&format!("{prefix}-"))
            .map(ToOwned::to_owned);
    }

    let prefix = format!("{prefix}-");
    let remainder = filename.strip_prefix(&prefix)?;
    let without_extension = remainder.strip_suffix(extension)?;
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

    use crate::runs::CreateRunRequest;

    use super::{
        CONFIG_PACKAGE_NAME, ENGINE_PACKAGE_NAME, ScopeSessionId, derive_session_identifier,
        parse_artifact_version, public_session_path, session_absolute_path,
    };

    #[test]
    fn parses_runtime_versions_from_standard_filenames() {
        assert_eq!(
            parse_artifact_version("ade_engine-0.1.0-py3-none-any.whl", "ade_engine", ".whl"),
            Some("0.1.0".to_string())
        );
        assert_eq!(
            parse_artifact_version("python-3.14.0-linux-x86_64.tar.gz", "python", ".tar.gz"),
            Some("3.14.0".to_string())
        );
        assert_eq!(
            parse_artifact_version("ade-session-agent", "ade-session-agent", ""),
            Some("dev".to_string())
        );
    }

    #[test]
    fn session_identifiers_are_stable_and_scope_specific() {
        let first = ScopeSessionId(derive_session_identifier("secret", "workspace-a:config-v1"));
        let second = ScopeSessionId(derive_session_identifier("secret", "workspace-a:config-v1"));
        let third = ScopeSessionId(derive_session_identifier("secret", "workspace-b:config-v1"));

        assert_eq!(first, second);
        assert_ne!(first, third);
        assert!(first.as_str().starts_with("cfg-"));
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
    fn public_session_paths_must_be_relative_and_not_internal() {
        assert_eq!(public_session_path("input.csv").unwrap(), "input.csv");
        assert_eq!(
            public_session_path("runs/run-1/output/input.xlsx").unwrap(),
            "runs/run-1/output/input.xlsx"
        );
        assert!(public_session_path("/input.csv").is_err());
        assert!(public_session_path("../input.csv").is_err());
    }

    #[test]
    fn session_absolute_paths_resolve_under_mnt_data() {
        assert_eq!(
            session_absolute_path(".ade/bin/ade-session-agent"),
            "/mnt/data/.ade/bin/ade-session-agent"
        );
    }

    #[test]
    fn package_names_remain_stable() {
        assert_eq!(CONFIG_PACKAGE_NAME, "ade-config");
        assert_eq!(ENGINE_PACKAGE_NAME, "ade-engine");
    }
}
