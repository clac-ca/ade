use std::{
    fs,
    path::{Path, PathBuf},
};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
    scope::Scope,
};

const DEFAULT_SANDBOX_ENVIRONMENT_ARCHIVE_PATH: &str = ".package/sandbox-environment.tar.gz";
const SANDBOX_CONFIG_ROOT_PATH: &str = "/mnt/data/ade/configs";
const PYTHON_EXECUTABLE_PATH: &str = "/app/ade/python/current/bin/python3";
const ADE_EXECUTABLE_PATH: &str = "/app/ade/python/current/bin/ade";
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

#[derive(Clone, Debug)]
pub(crate) struct SandboxAssets {
    archive: FileArtifact,
    sandbox_secret: String,
}

impl SandboxAssets {
    pub(crate) fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        let archive_path = read_optional_trimmed_string(env, SANDBOX_ENVIRONMENT_ARCHIVE_ENV_NAME)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SANDBOX_ENVIRONMENT_ARCHIVE_PATH));
        Self::from_paths(archive_path, env)
    }

    pub(crate) fn from_paths(archive_path: PathBuf, env: &EnvBag) -> Result<Self, AppError> {
        Ok(Self {
            archive: resolve_file_from_path(
                "sandbox environment archive",
                Some(".tar.gz"),
                &archive_path,
            )?,
            sandbox_secret: read_sandbox_secret(env)?,
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

    pub(crate) fn config_mount_directory(&self, scope: &Scope) -> String {
        format!(
            "{}/{}/{}",
            SANDBOX_CONFIG_ROOT_PATH, scope.workspace_id, scope.config_version_id
        )
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
    Ok(
        read_optional_trimmed_string(env, SANDBOX_SECRET_ENV_NAME).unwrap_or_else(|| {
            tracing::warn!(
                environment_variable = SANDBOX_SECRET_ENV_NAME,
                "Sandbox environment secret is not configured; generating a process-local fallback."
            );
            generate_sandbox_secret()
        }),
    )
}

fn generate_sandbox_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
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
    fn resolves_archive_and_mounted_paths_from_conventional_paths() {
        let tempdir = tempdir().unwrap();
        let archive_path = tempdir.path().join("sandbox-environment.tar.gz");
        write_archive(&archive_path);

        let env = [(
            super::SANDBOX_SECRET_ENV_NAME.to_string(),
            "secret".to_string(),
        )]
        .into_iter()
        .collect();
        let assets = SandboxAssets::from_paths(archive_path, &env).unwrap();

        assert_eq!(assets.setup_script_path(), "/app/ade/bin/setup.sh");
        assert_eq!(assets.archive_filename(), "sandbox-environment.tar.gz");
        assert_eq!(
            assets.archive_remote_path(),
            "/mnt/data/sandbox-environment.tar.gz"
        );
        assert_eq!(
            assets.ade_executable_path(),
            "/app/ade/python/current/bin/ade"
        );
        assert_eq!(
            assets.config_mount_directory(&crate::scope::Scope {
                workspace_id: "workspace-a".to_string(),
                config_version_id: "config-v1".to_string(),
            }),
            "/mnt/data/ade/configs/workspace-a/config-v1"
        );
        assert!(!assets.environment_revision().is_empty());
    }

    #[test]
    fn missing_secret_generates_process_local_fallback() {
        let tempdir = tempdir().unwrap();
        let archive_path = tempdir.path().join("sandbox-environment.tar.gz");
        write_archive(&archive_path);

        let first = SandboxAssets::from_paths(archive_path.clone(), &Default::default()).unwrap();
        let second = SandboxAssets::from_paths(archive_path, &Default::default()).unwrap();

        assert_eq!(first.sandbox_secret().len(), 64);
        assert_eq!(second.sandbox_secret().len(), 64);
        assert_ne!(first.sandbox_secret(), second.sandbox_secret());
        assert!(
            first
                .sandbox_secret()
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
        );
        assert!(
            second
                .sandbox_secret()
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
        );
    }
}
