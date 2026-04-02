use std::{
    fs,
    path::{Path, PathBuf},
};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
};

const CONFIG_FIXTURE_ROOT_ENV_NAME: &str = "ADE_CONFIG_FIXTURE_ROOT";
const DEFAULT_CONFIG_FIXTURE_ROOT: &str = ".package/configs";
const DEFAULT_SANDBOX_ENVIRONMENT_ARCHIVE_PATH: &str = ".package/sandbox-environment.tar.gz";
const LEGACY_SANDBOX_SECRET_ENV_NAME: &str = "ADE_SCOPE_SESSION_SECRET";
const MOUNTED_CONFIG_DIRECTORY_PATH: &str = "/mnt/data/ade/config/current";
const PYTHON_EXECUTABLE_PATH: &str = "/mnt/data/ade/python/current/bin/python3";
const ADE_EXECUTABLE_PATH: &str = "/mnt/data/ade/python/current/bin/ade";
const ROOT_PATH: &str = "/mnt/data";
const SANDBOX_ENVIRONMENT_ARCHIVE_ENV_NAME: &str = "ADE_SANDBOX_ENVIRONMENT_ARCHIVE_PATH";
const SANDBOX_SECRET_ENV_NAME: &str = "ADE_SANDBOX_ENVIRONMENT_SECRET";
const SETUP_SCRIPT_PATH: &str = "/app/ade/bin/setup.sh";
const CONNECTOR_PATH: &str = "/app/ade/bin/reverse-connect";
const BASE_WHEELHOUSE_PATH: &str = "/app/ade/wheelhouse/base";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SandboxId(String);

impl SandboxId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConfigPackage {
    pub(crate) filename: String,
    pub(crate) install_revision: String,
    pub(crate) local_path: PathBuf,
    pub(crate) mounted_directory_path: String,
    pub(crate) mounted_path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct SandboxAssets {
    archive: FileArtifact,
    config_fixture_root: PathBuf,
    sandbox_secret: String,
}

impl SandboxAssets {
    pub(crate) fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let archive_path = read_optional_trimmed_string(env, SANDBOX_ENVIRONMENT_ARCHIVE_ENV_NAME)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SANDBOX_ENVIRONMENT_ARCHIVE_PATH));
        let config_fixture_root = read_optional_trimmed_string(env, CONFIG_FIXTURE_ROOT_ENV_NAME)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FIXTURE_ROOT));

        Self::from_paths(archive_path, config_fixture_root, env)
    }

    pub(crate) fn from_paths(
        archive_path: PathBuf,
        config_fixture_root: PathBuf,
        env: &EnvBag,
    ) -> Result<Self, AppError> {
        Ok(Self {
            archive: resolve_file_from_path(
                "sandbox environment archive",
                Some(".tar.gz"),
                &archive_path,
            )?,
            config_fixture_root,
            sandbox_secret: read_sandbox_secret(env)?,
        })
    }

    pub(crate) fn config_for_scope(&self, scope: &Scope) -> Result<ConfigPackage, AppError> {
        let artifact = resolve_config_fixture(&self.config_fixture_root, scope)?;
        Ok(ConfigPackage {
            filename: artifact.filename.clone(),
            install_revision: artifact.revision,
            local_path: artifact.path,
            mounted_directory_path: MOUNTED_CONFIG_DIRECTORY_PATH.to_string(),
            mounted_path: format!("{MOUNTED_CONFIG_DIRECTORY_PATH}/{}", artifact.filename),
        })
    }

    pub(crate) fn connector_path(&self) -> &'static str {
        CONNECTOR_PATH
    }

    pub(crate) fn archive_content_type(&self) -> &'static str {
        "application/gzip"
    }

    pub(crate) fn archive_filename(&self) -> &str {
        &self.archive.filename
    }

    pub(crate) fn archive_local_path(&self) -> &Path {
        &self.archive.path
    }

    pub(crate) fn archive_remote_path(&self) -> String {
        format!("{ROOT_PATH}/{}", self.archive.filename)
    }

    pub(crate) fn environment_revision(&self) -> &str {
        &self.archive.revision
    }

    pub(crate) fn base_wheelhouse_path(&self) -> &'static str {
        BASE_WHEELHOUSE_PATH
    }

    pub(crate) fn python_executable_path(&self) -> &'static str {
        PYTHON_EXECUTABLE_PATH
    }

    pub(crate) fn ade_executable_path(&self) -> &'static str {
        ADE_EXECUTABLE_PATH
    }

    pub(crate) fn root_path(&self) -> &'static str {
        ROOT_PATH
    }

    pub(crate) fn sandbox_id(&self, scope: &Scope) -> SandboxId {
        SandboxId(derive_sandbox_identifier(
            &self.sandbox_secret,
            &format!("{}:{}", scope.workspace_id, scope.config_version_id),
        ))
    }

    pub(crate) fn sandbox_secret(&self) -> &str {
        &self.sandbox_secret
    }

    pub(crate) fn setup_script_path(&self) -> &'static str {
        SETUP_SCRIPT_PATH
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileArtifact {
    filename: String,
    path: PathBuf,
    revision: String,
}

fn read_sandbox_secret(env: &EnvBag) -> Result<String, AppError> {
    read_optional_trimmed_string(env, SANDBOX_SECRET_ENV_NAME)
        .or_else(|| read_optional_trimmed_string(env, LEGACY_SANDBOX_SECRET_ENV_NAME))
        .ok_or_else(|| {
            AppError::config(format!(
                "Missing required environment variable: {SANDBOX_SECRET_ENV_NAME}"
            ))
        })
}

fn resolve_config_fixture(config_root: &Path, scope: &Scope) -> Result<FileArtifact, AppError> {
    let directory = config_root
        .join(&scope.workspace_id)
        .join(&scope.config_version_id);
    resolve_first_artifact_with_extension(&directory, ".whl").map_err(|error| match error {
        AppError::NotFound { .. } => AppError::not_found(format!(
            "Config version '{}' for workspace '{}' was not found in '{}'.",
            scope.config_version_id,
            scope.workspace_id,
            directory.display()
        )),
        _ => error,
    })
}

fn resolve_first_artifact_with_extension(
    directory: &Path,
    extension: &str,
) -> Result<FileArtifact, AppError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        AppError::io_with_source(format!("Failed to read '{}'.", directory.display()), error)
    })?;
    let mut artifacts = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    artifacts.sort();

    let Some(path) = artifacts
        .into_iter()
        .find(|path| path.to_string_lossy().ends_with(extension))
    else {
        return Err(AppError::not_found(format!(
            "No runtime artifact ending with '{extension}' was found in '{}'.",
            directory.display()
        )));
    };

    resolve_file_from_path("config fixture", Some(extension), &path)
}

fn resolve_file_from_path(
    source_name: &str,
    required_extension: Option<&str>,
    path: &Path,
) -> Result<FileArtifact, AppError> {
    if !path.is_file() {
        return Err(AppError::config(format!(
            "Runtime artifact configured by {source_name} was not found at '{}'.",
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
    if let Some(required_extension) = required_extension
        && !filename.ends_with(required_extension)
    {
        return Err(AppError::config(format!(
            "Runtime artifact '{}' must end with '{required_extension}'.",
            path.display()
        )));
    }

    Ok(FileArtifact {
        filename,
        path: resolved_path.clone(),
        revision: file_revision(&resolved_path)?,
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

fn file_revision(path: &Path) -> Result<String, AppError> {
    let content = fs::read(path).map_err(|error| {
        AppError::io_with_source(
            format!("Failed to read runtime artifact '{}'.", path.display()),
            error,
        )
    })?;
    Ok(hex::encode(Sha256::digest(content)))
}

fn derive_sandbox_identifier(secret: &str, scope: &str) -> String {
    type HmacSha256 = Hmac<Sha256>;

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts arbitrary key lengths");
    mac.update(scope.as_bytes());
    let digest = hex::encode(mac.finalize().into_bytes());
    format!("cfg-{}", &digest[..32])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use flate2::{Compression, write::GzEncoder};
    use tar::Builder;
    use tempfile::tempdir;

    use super::{SandboxAssets, SandboxId, derive_sandbox_identifier};

    fn write_archive(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        let content = b"connector";
        header.set_path("app/ade/bin/reverse-connect").unwrap();
        header.set_mode(0o755);
        header.set_size(content.len() as u64);
        header.set_cksum();
        archive.append(&header, &content[..]).unwrap();
        archive.finish().unwrap();
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap();
    }

    #[test]
    fn sandbox_identifiers_are_stable_and_scope_specific() {
        let first = SandboxId(derive_sandbox_identifier("secret", "workspace-a:config-v1"));
        let second = SandboxId(derive_sandbox_identifier("secret", "workspace-a:config-v1"));
        let third = SandboxId(derive_sandbox_identifier("secret", "workspace-b:config-v1"));

        assert_eq!(first, second);
        assert_ne!(first, third);
        assert!(first.as_str().starts_with("cfg-"));
    }

    #[test]
    fn resolves_archive_and_config_fixture_from_conventional_paths() {
        let tempdir = tempdir().unwrap();
        let archive_path = tempdir.path().join("sandbox-environment.tar.gz");
        let config_root = tempdir.path().join("configs");
        std::fs::create_dir_all(config_root.join("workspace-a/config-v1")).unwrap();
        write_archive(&archive_path);
        std::fs::write(
            config_root.join("workspace-a/config-v1/ade_config-0.1.0-py3-none-any.whl"),
            b"config",
        )
        .unwrap();

        let env = [(
            super::SANDBOX_SECRET_ENV_NAME.to_string(),
            "secret".to_string(),
        )]
        .into_iter()
        .collect();
        let assets = SandboxAssets::from_paths(archive_path, config_root, &env).unwrap();
        let config = assets
            .config_for_scope(&crate::scope::Scope {
                workspace_id: "workspace-a".to_string(),
                config_version_id: "config-v1".to_string(),
            })
            .unwrap();

        assert_eq!(assets.setup_script_path(), "/app/ade/bin/setup.sh");
        assert_eq!(assets.archive_filename(), "sandbox-environment.tar.gz");
        assert_eq!(
            assets.archive_remote_path(),
            "/mnt/data/sandbox-environment.tar.gz"
        );
        assert_eq!(
            assets.ade_executable_path(),
            "/mnt/data/ade/python/current/bin/ade"
        );
        assert_eq!(
            config.mounted_path,
            "/mnt/data/ade/config/current/ade_config-0.1.0-py3-none-any.whl"
        );
        assert!(!assets.environment_revision().is_empty());
    }
}
