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

use super::client::{PythonExecution, SessionFile, SessionOperationResult, SessionPoolClient};

const CONFIG_PACKAGE_NAME: &str = "ade-config";
const CONFIG_TARGETS_ENV_NAME: &str = "ADE_CONFIG_TARGETS";
const ENGINE_PACKAGE_NAME: &str = "ade-engine";
const ENGINE_WHEEL_ENV_NAME: &str = "ADE_ENGINE_WHEEL_PATH";
const INSTALL_LOCK_SESSION_FILENAME: &str = ".ade-session-install.lock";
const RUNS_ROOT: &str = "runs";
const SESSION_ROOT: &str = "/mnt/data";
const SESSION_SECRET_ENV_NAME: &str = "ADE_SESSION_SECRET";

#[derive(Clone, Debug, PartialEq, Eq)]
struct PackageWheel {
    bytes: Vec<u8>,
    filename: String,
    version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRuntimeArtifacts {
    pub(crate) config_filename: String,
    pub(crate) config_package_name: &'static str,
    pub(crate) config_version: String,
    pub(crate) config_wheel_bytes: Vec<u8>,
    pub(crate) engine_filename: String,
    pub(crate) engine_package_name: &'static str,
    pub(crate) engine_version: String,
    pub(crate) engine_wheel_bytes: Vec<u8>,
    pub(crate) install_lock_path: String,
    pub(crate) runs_root: String,
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
        Ok(SessionRuntimeArtifacts {
            config_filename: config.filename.clone(),
            config_package_name: CONFIG_PACKAGE_NAME,
            config_version: config.version.clone(),
            config_wheel_bytes: config.bytes.clone(),
            engine_filename: self.engine.filename.clone(),
            engine_package_name: ENGINE_PACKAGE_NAME,
            engine_version: self.engine.version.clone(),
            engine_wheel_bytes: self.engine.bytes.clone(),
            install_lock_path: format!("{SESSION_ROOT}/{INSTALL_LOCK_SESSION_FILENAME}"),
            runs_root: format!("{SESSION_ROOT}/{RUNS_ROOT}"),
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
                &self.session_identifier(scope),
                public_session_path(path)?,
                content_type,
                content,
            )
            .await
    }

    pub(crate) async fn execute_inline_python_detailed(
        &self,
        scope: &Scope,
        code: String,
        timeout_in_seconds: Option<u64>,
    ) -> Result<SessionOperationResult<PythonExecution>, AppError> {
        self.client
            .execute(&self.session_identifier(scope), code, timeout_in_seconds)
            .await
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

    use crate::{runs::CreateRunRequest, scope::Scope};

    use super::{
        CONFIG_PACKAGE_NAME, ENGINE_PACKAGE_NAME, derive_session_identifier, parse_wheel_version,
        public_session_path,
    };
    use crate::session::python::{RUN_SENTINEL_PREFIX, RunPythonConfig, build_run_code};

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
    fn run_request_denies_unknown_fields() {
        let error = serde_json::from_value::<CreateRunRequest>(json!({
            "file": "input.csv",
        }))
        .unwrap_err()
        .to_string();

        assert!(error.contains("unknown field"));
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
