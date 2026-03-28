use std::{collections::BTreeMap, path::PathBuf};

use crate::error::AppError;

pub const DEFAULT_DEV_HOST: &str = "127.0.0.1";
pub const DEFAULT_RUNTIME_HOST: &str = "0.0.0.0";
pub const DEFAULT_PORT: u16 = 8000;
pub const DEFAULT_PROBE_INTERVAL_MS: u64 = 5_000;
pub const DEFAULT_READINESS_STALE_AFTER_MS: u64 = 15_000;
pub const SQL_CONNECTION_STRING_NAME: &str = "AZURE_SQL_CONNECTIONSTRING";
pub const SERVICE_NAME: &str = "ade";
pub const SERVICE_VERSION: &str = match option_env!("ADE_PLATFORM_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

pub type EnvBag = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    pub sql_connection_string: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationConfig {
    pub sql_connection_string: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimePaths {
    pub web_root: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadConfigOptions {
    pub require_sql: bool,
}

pub fn current_env() -> EnvBag {
    std::env::vars().collect()
}

pub fn default_runtime_paths() -> RuntimePaths {
    RuntimePaths {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(entries: &[(&str, &str)]) -> EnvBag {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn reads_optional_sql_without_requirement() {
        let config = read_config(&env(&[]), ReadConfigOptions::default()).unwrap();

        assert_eq!(config.sql_connection_string, None);
    }

    #[test]
    fn requires_sql_when_requested() {
        let error = read_config(&env(&[]), ReadConfigOptions { require_sql: true }).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Missing required environment variable: AZURE_SQL_CONNECTIONSTRING"
        );
    }

    #[test]
    fn reads_trimmed_sql_when_present() {
        let config = read_config(
            &env(&[("AZURE_SQL_CONNECTIONSTRING", " Server=sql; ")]),
            ReadConfigOptions { require_sql: true },
        )
        .unwrap();

        assert_eq!(config.sql_connection_string.as_deref(), Some("Server=sql;"));
    }
}
