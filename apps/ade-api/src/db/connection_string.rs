use std::collections::BTreeMap;

use tiberius::Config;

use crate::error::AppError;

#[derive(Clone, Debug)]
pub(super) struct SqlConnectionSettings {
    pub(super) authentication: SqlAuthenticationMode,
    pub(super) base_config: Config,
    pub(super) database_name: String,
}

impl SqlConnectionSettings {
    pub(super) fn parse(connection_string: &str) -> Result<Self, AppError> {
        let fields = parse_connection_fields(connection_string)?;
        let authentication = parse_authentication_mode(&fields)?;
        let database_name = read_required_field(&fields, &["database", "initialcatalog"])?;

        if authentication == SqlAuthenticationMode::SqlPassword {
            let user_id = read_optional_field(&fields, &["userid", "uid", "user", "username"]);
            let password = read_optional_field(&fields, &["password", "pwd"]);

            if user_id.is_none() || password.is_none() {
                return Err(AppError::config(
                    "SQL authentication requires both User ID and Password.",
                ));
            }
        }

        let base_config = Config::from_ado_string(connection_string).map_err(|error| {
            AppError::config_with_source("Failed to parse SQL connection string.", error)
        })?;

        Ok(Self {
            authentication,
            base_config,
            database_name,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SqlAuthenticationMode {
    ActiveDirectoryDefault { client_id: Option<String> },
    ManagedIdentity { client_id: Option<String> },
    SqlPassword,
}

fn parse_connection_fields(connection_string: &str) -> Result<BTreeMap<String, String>, AppError> {
    let mut fields = BTreeMap::new();
    let mut segment = String::new();
    let mut in_braces = false;
    let chars: Vec<char> = connection_string.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];

        match character {
            ';' if !in_braces => {
                push_connection_field(&mut fields, &segment)?;
                segment.clear();
            }
            '{' => segment.push(character),
            '}' => {
                if in_braces && chars.get(index + 1) == Some(&'}') {
                    segment.push('}');
                    index += 1;
                } else {
                    segment.push(character);
                    in_braces = false;
                }
            }
            _ => {
                if character == '{' {
                    in_braces = true;
                }

                segment.push(character);
            }
        }

        if character == '{' {
            in_braces = true;
        }

        index += 1;
    }

    push_connection_field(&mut fields, &segment)?;
    Ok(fields)
}

fn push_connection_field(
    fields: &mut BTreeMap<String, String>,
    segment: &str,
) -> Result<(), AppError> {
    let trimmed = segment.trim();

    if trimmed.is_empty() {
        return Ok(());
    }

    let (key, raw_value) = trimmed.split_once('=').ok_or_else(|| {
        AppError::config(format!("Invalid SQL connection string segment: {trimmed}"))
    })?;

    fields.insert(normalize_field_name(key), parse_field_value(raw_value));
    Ok(())
}

fn parse_field_value(value: &str) -> String {
    let trimmed = value.trim();

    if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() >= 2 {
        return trimmed[1..trimmed.len() - 1]
            .replace("}}", "}")
            .replace("{{", "{");
    }

    trimmed.to_string()
}

fn normalize_field_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_authentication_mode(
    fields: &BTreeMap<String, String>,
) -> Result<SqlAuthenticationMode, AppError> {
    let authentication = fields
        .get("authentication")
        .map(|value| normalize_field_name(value));
    let client_id = read_optional_field(fields, &["userid", "uid", "user", "username"]);

    match authentication.as_deref() {
        None | Some("sqlpassword") => Ok(SqlAuthenticationMode::SqlPassword),
        Some("activedirectorydefault") => {
            Ok(SqlAuthenticationMode::ActiveDirectoryDefault { client_id })
        }
        Some("activedirectorymanagedidentity") => {
            Ok(SqlAuthenticationMode::ManagedIdentity { client_id })
        }
        Some(other) => Err(AppError::config(format!(
            "Unsupported SQL authentication mode: {other}. Supported values: SqlPassword, ActiveDirectoryManagedIdentity, ActiveDirectoryDefault."
        ))),
    }
}

pub(super) fn read_optional_field(
    fields: &BTreeMap<String, String>,
    names: &[&str],
) -> Option<String> {
    names
        .iter()
        .find_map(|name| fields.get(*name))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn read_required_field(
    fields: &BTreeMap<String, String>,
    names: &[&str],
) -> Result<String, AppError> {
    read_optional_field(fields, names).ok_or_else(|| {
        AppError::config(format!(
            "SQL connection string is missing one of: {}",
            names.join(", ")
        ))
    })
}

pub(super) fn quote_sql_identifier(value: &str) -> String {
    format!("[{}]", value.replace(']', "]]"))
}

#[cfg(test)]
mod tests {
    use super::{SqlAuthenticationMode, SqlConnectionSettings};

    #[test]
    fn parses_sql_password_connection_strings() {
        let settings = SqlConnectionSettings::parse(
            "Server=tcp:localhost,1433;Database=ade;User ID=sa;Password=Secret123!;Encrypt=false;TrustServerCertificate=true",
        )
        .unwrap();

        assert_eq!(settings.authentication, SqlAuthenticationMode::SqlPassword);
        assert_eq!(settings.database_name, "ade");
    }

    #[test]
    fn parses_managed_identity_connection_strings() {
        let settings = SqlConnectionSettings::parse(
            "Data Source=tcp:ade.database.windows.net,1433;Initial Catalog=sqldb-ade;User ID=client-id;Authentication=ActiveDirectoryManagedIdentity;Encrypt=True;TrustServerCertificate=False",
        )
        .unwrap();

        assert_eq!(
            settings.authentication,
            SqlAuthenticationMode::ManagedIdentity {
                client_id: Some("client-id".to_string())
            }
        );
        assert_eq!(settings.database_name, "sqldb-ade");
    }

    #[test]
    fn parses_managed_identity_connection_strings_without_user_assigned_id() {
        let settings = SqlConnectionSettings::parse(
            "Data Source=tcp:ade.database.windows.net,1433;Initial Catalog=sqldb-ade;Authentication=ActiveDirectoryManagedIdentity;Encrypt=True;TrustServerCertificate=False",
        )
        .unwrap();

        assert_eq!(
            settings.authentication,
            SqlAuthenticationMode::ManagedIdentity { client_id: None }
        );
        assert_eq!(settings.database_name, "sqldb-ade");
    }

    #[test]
    fn parses_default_connection_strings_without_user_assigned_id() {
        let settings = SqlConnectionSettings::parse(
            "Data Source=tcp:ade.database.windows.net,1433;Initial Catalog=sqldb-ade;Authentication=ActiveDirectoryDefault;Encrypt=True;TrustServerCertificate=False",
        )
        .unwrap();

        assert_eq!(
            settings.authentication,
            SqlAuthenticationMode::ActiveDirectoryDefault { client_id: None }
        );
        assert_eq!(settings.database_name, "sqldb-ade");
    }

    #[test]
    fn parses_default_connection_strings_with_user_assigned_id() {
        let settings = SqlConnectionSettings::parse(
            "Data Source=tcp:ade.database.windows.net,1433;Initial Catalog=sqldb-ade;User ID=client-id;Authentication=ActiveDirectoryDefault;Encrypt=True;TrustServerCertificate=False",
        )
        .unwrap();

        assert_eq!(
            settings.authentication,
            SqlAuthenticationMode::ActiveDirectoryDefault {
                client_id: Some("client-id".to_string())
            }
        );
        assert_eq!(settings.database_name, "sqldb-ade");
    }

    #[test]
    fn rejects_sql_password_without_credentials() {
        let error = SqlConnectionSettings::parse(
            "Server=localhost;Database=ade;Encrypt=false;TrustServerCertificate=true",
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "SQL authentication requires both User ID and Password."
        );
    }
}
