use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

pub const DEFAULT_DEV_HOST: &str = "127.0.0.1";
pub const DEFAULT_RUNTIME_HOST: &str = "0.0.0.0";
pub const DEFAULT_PORT: u16 = 8000;
pub const DEFAULT_PROBE_INTERVAL_MS: u64 = 5_000;
pub const DEFAULT_READINESS_STALE_AFTER_MS: u64 = 15_000;
pub const SQL_CONNECTION_STRING_NAME: &str = "AZURE_SQL_CONNECTIONSTRING";

pub type EnvBag = BTreeMap<String, String>;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    #[serde(rename = "builtAt")]
    pub built_at: String,
    #[serde(rename = "gitSha")]
    pub git_sha: String,
    pub service: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct VersionInfo {
    #[serde(flatten)]
    pub build_info: BuildInfo,
    #[serde(rename = "runtimeVersion")]
    pub runtime_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    pub build_info: BuildInfo,
    pub sql_connection_string: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationConfig {
    pub sql_connection_string: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimePaths {
    pub build_info_path: PathBuf,
    pub web_root: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadConfigOptions {
    pub build_info_path: Option<PathBuf>,
    pub require_sql: bool,
}

pub fn current_env() -> EnvBag {
    std::env::vars().collect()
}

pub fn default_runtime_paths() -> RuntimePaths {
    RuntimePaths {
        build_info_path: PathBuf::from("dist/build-info.json"),
        web_root: PathBuf::from("public"),
    }
}

pub fn read_config(env: &EnvBag, options: ReadConfigOptions) -> Result<AppConfig, AppError> {
    let sql_connection_string = if options.require_sql {
        Some(read_required_trimmed_string(
            env,
            SQL_CONNECTION_STRING_NAME,
        )?)
    } else {
        read_optional_trimmed_string(env, SQL_CONNECTION_STRING_NAME)
    };

    Ok(AppConfig {
        build_info: read_build_info(env, options.build_info_path.as_deref())?,
        sql_connection_string,
    })
}

pub fn read_migration_config(env: &EnvBag) -> Result<MigrationConfig, AppError> {
    Ok(MigrationConfig {
        sql_connection_string: read_required_trimmed_string(env, SQL_CONNECTION_STRING_NAME)?,
    })
}

pub fn read_optional_trimmed_string(env: &EnvBag, name: &str) -> Option<String> {
    env.get(name)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn is_production(env: &EnvBag) -> bool {
    matches!(
        read_optional_trimmed_string(env, "NODE_ENV").as_deref(),
        Some("production")
    )
}

fn read_required_trimmed_string(env: &EnvBag, name: &str) -> Result<String, AppError> {
    read_optional_trimmed_string(env, name)
        .ok_or_else(|| AppError::config(format!("Missing required environment variable: {name}")))
}

fn read_build_info(env: &EnvBag, build_info_path: Option<&Path>) -> Result<BuildInfo, AppError> {
    let default_paths = default_runtime_paths();
    let path = build_info_path.unwrap_or(default_paths.build_info_path.as_path());

    if path.exists() {
        let payload = fs::read_to_string(path).map_err(|error| {
            AppError::io_with_source(
                format!("Failed to read build info at {}.", path.display()),
                error,
            )
        })?;

        let build_info: BuildInfo = serde_json::from_str(&payload).map_err(|error| {
            AppError::config_with_source(
                format!("ADE build info at {} must be valid JSON.", path.display()),
                error,
            )
        })?;

        validate_build_info(build_info)
    } else if is_production(env) {
        Err(AppError::config(format!(
            "Missing ADE build info at {}.",
            path.display()
        )))
    } else {
        Ok(read_development_build_info())
    }
}

fn validate_build_info(build_info: BuildInfo) -> Result<BuildInfo, AppError> {
    for (name, value) in [
        ("service", build_info.service.as_str()),
        ("version", build_info.version.as_str()),
        ("gitSha", build_info.git_sha.as_str()),
        ("builtAt", build_info.built_at.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(AppError::config(format!(
                "ADE build info field \"{name}\" must be a non-empty string."
            )));
        }
    }

    if build_info.service != "ade" {
        return Err(AppError::config(
            "ADE build info service must be \"ade\".".to_string(),
        ));
    }

    Ok(build_info)
}

fn read_development_build_info() -> BuildInfo {
    BuildInfo {
        built_at: read_git_value(["show", "--no-patch", "--format=%cI", "HEAD"])
            .unwrap_or_else(|| "dev".to_string()),
        git_sha: read_git_value(["rev-parse", "HEAD"]).unwrap_or_else(|| "dev".to_string()),
        service: "ade".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn read_git_value<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();

    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn env(entries: &[(&str, &str)]) -> EnvBag {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn reads_packaged_build_info() {
        let temp_dir = tempdir().unwrap();
        let build_info_path = temp_dir.path().join("build-info.json");

        fs::write(
            &build_info_path,
            r#"{"builtAt":"2026-03-26T00:00:00.000Z","gitSha":"abc123","service":"ade","version":"1.2.3"}"#,
        )
        .unwrap();

        let config = read_config(
            &env(&[]),
            ReadConfigOptions {
                build_info_path: Some(build_info_path),
                require_sql: false,
            },
        )
        .unwrap();

        assert_eq!(
            config.build_info,
            BuildInfo {
                built_at: "2026-03-26T00:00:00.000Z".to_string(),
                git_sha: "abc123".to_string(),
                service: "ade".to_string(),
                version: "1.2.3".to_string(),
            }
        );
    }

    #[test]
    fn requires_sql_when_requested() {
        let error = read_config(
            &env(&[]),
            ReadConfigOptions {
                require_sql: true,
                ..ReadConfigOptions::default()
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Missing required environment variable: AZURE_SQL_CONNECTIONSTRING"
        );
    }

    #[test]
    fn fails_fast_in_production_without_build_info() {
        let error = read_config(
            &env(&[("NODE_ENV", "production")]),
            ReadConfigOptions {
                build_info_path: Some(PathBuf::from("does-not-exist.json")),
                require_sql: false,
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Missing ADE build info at does-not-exist.json."
        );
    }
}
