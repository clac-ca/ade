use std::{
    fs,
    path::{Path, PathBuf},
};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
};

const BASE_WHEELHOUSE_DIR: &str = "wheelhouse/base";
const CONNECTOR_BINARY_NAME: &str = "reverse-connect";
const CONNECTOR_LOCAL_PATH: &str = "bin/reverse-connect";
const CONNECTOR_SESSION_PATH: &str = "ade/bin/reverse-connect";
const CONFIG_LOCAL_DIR: &str = ".package/session-configs";
const CONFIG_SESSION_DIR: &str = "ade/config";
const DEFAULT_SESSION_BUNDLE_ROOT: &str = ".package/session-bundle";
const PREPARE_SCRIPT_LOCAL_PATH: &str = "bin/prepare.sh";
const PREPARE_SCRIPT_SESSION_PATH: &str = "ade/bin/prepare.sh";
const RUN_SCRIPT_LOCAL_PATH: &str = "bin/run.py";
const RUN_SCRIPT_SESSION_PATH: &str = "ade/bin/run.py";
const PYTHON_HOME_ROOT: &str = "/mnt/data/ade/python/current";
const PYTHON_TOOLCHAIN_LOCAL_DIR: &str = "python";
const PYTHON_TOOLCHAIN_SESSION_DIR: &str = "ade/python";
const SESSION_UPLOAD_ROOT: &str = "/app";
const SESSION_ROOT: &str = "/mnt/data";
const SCOPE_SESSION_SECRET_ENV_NAME: &str = "ADE_SCOPE_SESSION_SECRET";
const WHEELHOUSE_SESSION_DIR: &str = "ade/wheelhouse/base";

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
    pub(crate) run_script_path: String,
    pub(crate) session_root: String,
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
    config_root: PathBuf,
    connector_binary: FileArtifact,
    prepare_script_path: PathBuf,
    run_script_path: PathBuf,
    python_toolchain: FileArtifact,
    session_secret: String,
}

impl SessionBundleSource {
    pub(crate) fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        Self::from_paths(
            PathBuf::from(DEFAULT_SESSION_BUNDLE_ROOT),
            PathBuf::from(CONFIG_LOCAL_DIR),
            env,
        )
    }

    pub(crate) fn from_paths(
        bundle_root: PathBuf,
        config_root: PathBuf,
        env: &EnvBag,
    ) -> Result<Self, AppError> {
        let session_secret = read_optional_trimmed_string(env, SCOPE_SESSION_SECRET_ENV_NAME)
            .ok_or_else(|| {
                AppError::config(format!(
                    "Missing required environment variable: {SCOPE_SESSION_SECRET_ENV_NAME}"
                ))
            })?;
        let connector_binary = resolve_file_from_path(
            "session bundle root",
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
        let run_script_path = bundle_root.join(RUN_SCRIPT_LOCAL_PATH);
        if !run_script_path.is_file() {
            return Err(AppError::config(format!(
                "Session bundle run script was not found at '{}'.",
                run_script_path.display()
            )));
        }
        let python_toolchain = resolve_python_toolchain(&bundle_root)?;

        Ok(Self {
            base_wheels: resolve_base_wheels(&bundle_root)?,
            config_root,
            connector_binary,
            prepare_script_path,
            run_script_path,
            python_toolchain,
            session_secret,
        })
    }

    pub(crate) fn bundle_for_scope(&self, scope: &Scope) -> Result<SessionBundle, AppError> {
        let config = resolve_config_wheel(&self.config_root, scope)?;
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
                content_type: "text/x-python",
                local_path: self.run_script_path.clone(),
                session_path: RUN_SCRIPT_SESSION_PATH.to_string(),
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
            session_path: format!("{CONFIG_SESSION_DIR}/{}", config.filename),
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
            connector_path: session_upload_path(CONNECTOR_SESSION_PATH),
            files,
            prepare_revision: revision_parts.join("|"),
            prepare_script_path: session_upload_path(PREPARE_SCRIPT_SESSION_PATH),
            python_executable_path: format!("{PYTHON_HOME_ROOT}/bin/python3"),
            run_script_path: session_upload_path(RUN_SCRIPT_SESSION_PATH),
            session_root: SESSION_ROOT.to_string(),
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

fn session_upload_path(path: &str) -> String {
    format!("{SESSION_UPLOAD_ROOT}/{}", path.trim_start_matches('/'))
}

fn resolve_config_wheel(config_root: &Path, scope: &Scope) -> Result<FileArtifact, AppError> {
    let directory = config_root
        .join(&scope.workspace_id)
        .join(&scope.config_version_id);
    let entries = fs::read_dir(&directory).map_err(|error| {
        AppError::io_with_source(
            format!(
                "Failed to read the scope config directory '{}'.",
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
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("whl"))
    else {
        return Err(AppError::not_found(format!(
            "Config version '{}' for workspace '{}' was not found in '{}'.",
            scope.config_version_id,
            scope.workspace_id,
            directory.display()
        )));
    };

    resolve_file_from_path("scope config root", "ade_config", ".whl", &path)
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

    resolve_file_from_path("session bundle root", "python", ".tar.gz", &path)
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
            "Runtime artifact configured by the session bundle convention was not found at '{}'.",
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
    use tempfile::tempdir;

    use super::{
        ScopeSessionId, SessionBundleSource, derive_session_identifier, parse_artifact_version,
        session_upload_path,
    };

    #[test]
    fn parses_runtime_versions_from_standard_filenames() {
        assert_eq!(
            parse_artifact_version("ade_engine-0.1.0-py3-none-any.whl", "ade_engine", ".whl"),
            Some("0.1.0".to_string())
        );
        assert_eq!(
            parse_artifact_version("python-3.12.11-linux-x86_64.tar.gz", "python", ".tar.gz"),
            Some("3.12.11".to_string())
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
    fn session_upload_paths_resolve_under_app() {
        assert_eq!(
            session_upload_path("ade/bin/reverse-connect"),
            "/app/ade/bin/reverse-connect"
        );
    }

    #[test]
    fn resolves_scope_config_from_conventional_path() {
        let tempdir = tempdir().unwrap();
        let bundle_root = tempdir.path().join("bundle");
        let config_root = tempdir.path().join("configs");
        std::fs::create_dir_all(bundle_root.join("bin")).unwrap();
        std::fs::create_dir_all(bundle_root.join("python")).unwrap();
        std::fs::create_dir_all(bundle_root.join("wheelhouse/base")).unwrap();
        std::fs::create_dir_all(config_root.join("workspace-a/config-v1")).unwrap();
        std::fs::write(bundle_root.join("bin/reverse-connect"), b"connector").unwrap();
        std::fs::write(bundle_root.join("bin/prepare.sh"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(bundle_root.join("bin/run.py"), b"print('ok')\n").unwrap();
        std::fs::write(
            bundle_root.join("python/python-3.12.11-linux-x86_64.tar.gz"),
            b"toolchain",
        )
        .unwrap();
        std::fs::write(
            bundle_root.join("wheelhouse/base/ade_engine-0.1.0-py3-none-any.whl"),
            b"engine",
        )
        .unwrap();
        std::fs::write(
            config_root.join("workspace-a/config-v1/ade_config-0.1.0-py3-none-any.whl"),
            b"config",
        )
        .unwrap();

        let env = [("ADE_SCOPE_SESSION_SECRET".to_string(), "secret".to_string())]
            .into_iter()
            .collect();
        let bundle = SessionBundleSource::from_paths(bundle_root, config_root, &env)
            .unwrap()
            .bundle_for_scope(&crate::scope::Scope {
                workspace_id: "workspace-a".to_string(),
                config_version_id: "config-v1".to_string(),
            })
            .unwrap();

        assert_eq!(bundle.prepare_script_path, "/app/ade/bin/prepare.sh");
        assert_eq!(bundle.run_script_path, "/app/ade/bin/run.py");
        assert!(bundle.files.iter().any(|file| {
            file.session_path == "ade/config/ade_config-0.1.0-py3-none-any.whl"
        }));
    }
}
