use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
};

const BASE_WHEELHOUSE_DIR: &str = "wheelhouse/base";
const CONFIG_TARGETS_ENV_NAME: &str = "ADE_CONFIG_TARGETS";
const CONNECTOR_BINARY_NAME: &str = "reverse-connect";
const CONNECTOR_LOCAL_PATH: &str = "bin/reverse-connect";
const CONNECTOR_SESSION_PATH: &str = ".ade/bin/reverse-connect";
const DEFAULT_SESSION_BUNDLE_ROOT: &str = "/app/session-bundle";
const PREPARE_SCRIPT_LOCAL_PATH: &str = "bin/prepare.sh";
const PREPARE_SCRIPT_SESSION_PATH: &str = ".ade/bin/prepare.sh";
const PYTHON_HOME_ROOT: &str = "/mnt/data/.ade/python/current";
const PYTHON_TOOLCHAIN_LOCAL_DIR: &str = "python";
const PYTHON_TOOLCHAIN_SESSION_DIR: &str = ".ade/python";
const SESSION_BUNDLE_ROOT_ENV_NAME: &str = "ADE_SESSION_BUNDLE_ROOT";
const SESSION_ROOT: &str = "/mnt/data";
const SESSION_SECRET_ENV_NAME: &str = "ADE_SESSION_SECRET";
const WHEELHOUSE_SESSION_DIR: &str = ".ade/wheelhouse";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ScopeSessionId(String);

impl ScopeSessionId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BundleFile {
    pub(crate) content_type: &'static str,
    pub(crate) local_path: PathBuf,
    pub(crate) session_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionBundle {
    pub(crate) connector_path: String,
    pub(crate) files: Vec<BundleFile>,
    pub(crate) prepare_revision: String,
    pub(crate) prepare_script_path: String,
    pub(crate) python_executable_path: String,
    pub(crate) python_home_path: String,
    pub(crate) python_toolchain_path: String,
    pub(crate) session_root: String,
    pub(crate) wheelhouse_path: String,
    pub(crate) config_wheel_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileArtifact {
    filename: String,
    path: PathBuf,
    version: String,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionBundleSource {
    base_wheels: Vec<FileArtifact>,
    config_targets: HashMap<Scope, FileArtifact>,
    connector_binary: FileArtifact,
    prepare_script_path: PathBuf,
    python_toolchain: FileArtifact,
    session_secret: String,
}

impl SessionBundleSource {
    pub(crate) fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let session_secret = read_optional_trimmed_string(env, SESSION_SECRET_ENV_NAME)
            .ok_or_else(|| {
                AppError::config(format!(
                    "Missing required environment variable: {SESSION_SECRET_ENV_NAME}"
                ))
            })?;
        let bundle_root = PathBuf::from(
            read_optional_trimmed_string(env, SESSION_BUNDLE_ROOT_ENV_NAME)
                .unwrap_or_else(|| DEFAULT_SESSION_BUNDLE_ROOT.to_string()),
        );

        let connector_binary = resolve_file_from_path(
            SESSION_BUNDLE_ROOT_ENV_NAME,
            CONNECTOR_BINARY_NAME,
            if cfg!(windows) { ".exe" } else { "" },
            &bundle_root.join(CONNECTOR_LOCAL_PATH),
        )?;
        let prepare_script_path = bundle_root.join(PREPARE_SCRIPT_LOCAL_PATH);
        if !prepare_script_path.is_file() {
            return Err(AppError::config(format!(
                "Session bundle prepare script was not found at '{}'.",
                prepare_script_path.display()
            )));
        }
        let python_toolchain = resolve_python_toolchain(&bundle_root)?;

        Ok(Self {
            base_wheels: resolve_base_wheels(&bundle_root)?,
            config_targets: resolve_config_targets(env)?,
            connector_binary,
            prepare_script_path,
            python_toolchain,
            session_secret,
        })
    }

    pub(crate) fn bundle_for_scope(&self, scope: &Scope) -> Result<SessionBundle, AppError> {
        let config = self.config_targets.get(scope).ok_or_else(|| {
            AppError::not_found(format!(
                "Config version '{}' for workspace '{}' is not configured.",
                scope.config_version_id, scope.workspace_id
            ))
        })?;
        let mut files = vec![
            BundleFile {
                content_type: "application/octet-stream",
                local_path: self.connector_binary.path.clone(),
                session_path: CONNECTOR_SESSION_PATH.to_string(),
            },
            BundleFile {
                content_type: "text/x-shellscript",
                local_path: self.prepare_script_path.clone(),
                session_path: PREPARE_SCRIPT_SESSION_PATH.to_string(),
            },
            BundleFile {
                content_type: "application/gzip",
                local_path: self.python_toolchain.path.clone(),
                session_path: format!(
                    "{PYTHON_TOOLCHAIN_SESSION_DIR}/{}",
                    self.python_toolchain.filename
                ),
            },
        ];
        files.extend(self.base_wheels.iter().map(|wheel| BundleFile {
            content_type: "application/octet-stream",
            local_path: wheel.path.clone(),
            session_path: format!("{WHEELHOUSE_SESSION_DIR}/{}", wheel.filename),
        }));
        files.push(BundleFile {
            content_type: "application/octet-stream",
            local_path: config.path.clone(),
            session_path: format!("{WHEELHOUSE_SESSION_DIR}/{}", config.filename),
        });

        let mut revision_parts = vec![
            format!("python={}", self.python_toolchain.version),
            format!("config={}", config.version),
        ];
        revision_parts.extend(
            self.base_wheels
                .iter()
                .map(|wheel| format!("{}={}", wheel.filename, wheel.version)),
        );

        Ok(SessionBundle {
            connector_path: session_absolute_path(CONNECTOR_SESSION_PATH),
            files,
            prepare_revision: revision_parts.join("|"),
            prepare_script_path: session_absolute_path(PREPARE_SCRIPT_SESSION_PATH),
            python_executable_path: format!("{PYTHON_HOME_ROOT}/bin/python3"),
            python_home_path: PYTHON_HOME_ROOT.to_string(),
            python_toolchain_path: session_absolute_path(&format!(
                "{PYTHON_TOOLCHAIN_SESSION_DIR}/{}",
                self.python_toolchain.filename
            )),
            session_root: SESSION_ROOT.to_string(),
            wheelhouse_path: session_absolute_path(WHEELHOUSE_SESSION_DIR),
            config_wheel_path: session_absolute_path(&format!(
                "{WHEELHOUSE_SESSION_DIR}/{}",
                config.filename
            )),
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigTargetEntry {
    workspace_id: String,
    config_version_id: String,
    wheel_path: PathBuf,
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

fn resolve_python_toolchain(bundle_root: &Path) -> Result<FileArtifact, AppError> {
    let directory = bundle_root.join(PYTHON_TOOLCHAIN_LOCAL_DIR);
    let entries = fs::read_dir(&directory).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to read the Python toolchain directory '{}'.",
                directory.display()
            ),
            error,
        )
    })?;
    let mut artifacts = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    artifacts.sort();

    let Some(path) = artifacts
        .into_iter()
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("gz"))
    else {
        return Err(AppError::config(format!(
            "No Python toolchain bundle was found in '{}'.",
            directory.display()
        )));
    };

    resolve_file_from_path(SESSION_BUNDLE_ROOT_ENV_NAME, "python", ".tar.gz", &path)
}

fn resolve_base_wheels(bundle_root: &Path) -> Result<Vec<FileArtifact>, AppError> {
    let directory = bundle_root.join(BASE_WHEELHOUSE_DIR);
    let entries = fs::read_dir(&directory).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to read the base wheelhouse from '{}'.",
                directory.display()
            ),
            error,
        )
    })?;
    let mut wheels = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .map(resolve_base_wheel)
        .collect::<Result<Vec<_>, _>>()?;
    wheels.sort_by(|left, right| left.filename.cmp(&right.filename));
    if wheels.is_empty() {
        return Err(AppError::config(format!(
            "The base wheelhouse in '{}' must include at least one wheel.",
            directory.display()
        )));
    }
    Ok(wheels)
}

fn resolve_base_wheel(path: PathBuf) -> Result<FileArtifact, AppError> {
    if !path.is_file() {
        return Err(AppError::config(format!(
            "Runtime artifact configured by {SESSION_BUNDLE_ROOT_ENV_NAME} was not found at '{}'.",
            path.display()
        )));
    }

    let resolved_path = fs::canonicalize(&path).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to resolve the runtime artifact from '{}'.",
                path.display()
            ),
            error,
        )
    })?;
    let filename = artifact_filename(&resolved_path)?;
    if !filename.ends_with(".whl") {
        return Err(AppError::config(format!(
            "Runtime artifact '{}' must end with '.whl'.",
            path.display()
        )));
    }

    Ok(FileArtifact {
        version: filename.clone(),
        filename,
        path: resolved_path,
    })
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
    let filename = artifact_filename(&resolved_path)?;
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

    Ok(FileArtifact {
        filename,
        path: resolved_path,
        version,
    })
}

fn artifact_filename(path: &Path) -> Result<String, AppError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            AppError::config(format!(
                "Runtime artifact path '{}' does not end with a valid filename.",
                path.display()
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

    use super::{
        ScopeSessionId, derive_session_identifier, parse_artifact_version, session_absolute_path,
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
            parse_artifact_version("reverse-connect", "reverse-connect", ""),
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
    fn session_absolute_paths_resolve_under_mnt_data() {
        assert_eq!(
            session_absolute_path(".ade/bin/reverse-connect"),
            "/mnt/data/.ade/bin/reverse-connect"
        );
    }

    #[test]
    fn config_target_shape_stays_stable() {
        let value = json!([
            {
                "workspaceId": "workspace-a",
                "configVersionId": "config-v1",
                "wheelPath": "/tmp/config.whl",
            }
        ]);

        assert_eq!(value[0]["workspaceId"], "workspace-a");
        assert_eq!(value[0]["configVersionId"], "config-v1");
    }
}
