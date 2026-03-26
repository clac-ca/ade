use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use azure_core::{
    Error as AzureError,
    credentials::{AccessToken, TokenCredential, TokenRequestOptions},
    error::ErrorKind as AzureErrorKind,
};
use azure_identity::{
    DeveloperToolsCredential, ManagedIdentityCredential, ManagedIdentityCredentialOptions,
    UserAssignedId, WorkloadIdentityCredential, WorkloadIdentityCredentialOptions,
};
use bb8::{ManageConnection, Pool};
use bb8_tiberius::ConnectionManager;
use thiserror::Error;
use tiberius::{AuthMethod, Config};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

use crate::error::AppError;

const SQL_TOKEN_SCOPE: &str = "https://database.windows.net/.default";
const MIGRATION_TABLE_NAME: &str = "schema_migrations";
const MIGRATION_TABLE_FQ_NAME: &str = "dbo.schema_migrations";
const ENSURE_MIGRATION_TABLE_QUERY: &str = "
IF OBJECT_ID(N'dbo.schema_migrations', N'U') IS NULL
BEGIN
  CREATE TABLE dbo.schema_migrations(
    version BIGINT NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    applied_on VARCHAR(255) NOT NULL,
    checksum VARCHAR(255) NOT NULL
  );
END
";

type SqlClient = bb8_tiberius::rt::Client;
type SqlTokenCredential = dyn TokenCredential + Send + Sync;

#[async_trait]
pub trait DatabaseProbe: Send + Sync {
    async fn ping(&self) -> Result<(), AppError>;
    async fn close(&self) -> Result<(), AppError>;
}

#[async_trait]
pub trait DatabaseConnector: Send + Sync {
    async fn connect(&self, connection_string: &str) -> Result<Arc<dyn DatabaseProbe>, AppError>;
}

#[derive(Debug, Default)]
pub struct LiveDatabaseConnector;

#[async_trait]
impl DatabaseConnector for LiveDatabaseConnector {
    async fn connect(&self, connection_string: &str) -> Result<Arc<dyn DatabaseProbe>, AppError> {
        Ok(Arc::new(Database::connect(connection_string).await?))
    }
}

#[derive(Clone)]
pub struct Database {
    pool: Pool<SqlConnectionManager>,
}

impl Database {
    pub async fn connect(connection_string: &str) -> Result<Self, AppError> {
        let settings = SqlConnectionSettings::parse(connection_string)?;
        ensure_database_exists(&settings).await?;
        let manager = SqlConnectionManager::from_settings(&settings)?;
        let pool = Pool::builder()
            .max_size(8)
            .build(manager)
            .await
            .map_err(|error| {
                AppError::database_with_source("Failed to initialize SQL pool.".to_string(), error)
            })?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl DatabaseProbe for Database {
    async fn ping(&self) -> Result<(), AppError> {
        let mut connection = self.pool.get().await.map_err(|error| {
            AppError::database_with_source("Failed to acquire SQL connection.".to_string(), error)
        })?;

        let stream = connection
            .simple_query("SELECT 1 AS value")
            .await
            .map_err(|error| {
                AppError::database_with_source("SQL ping failed.".to_string(), error)
            })?;
        drain_query_stream(stream).await.map_err(|error| {
            AppError::database_with_source("SQL ping failed.".to_string(), error)
        })?;

        Ok(())
    }

    async fn close(&self) -> Result<(), AppError> {
        Ok(())
    }
}

pub async fn run_migrations(connection_string: &str) -> Result<Vec<String>, AppError> {
    let settings = SqlConnectionSettings::parse(connection_string)?;
    ensure_database_exists(&settings).await?;

    let manager = SqlConnectionManager::from_settings(&settings)?;
    let mut client = manager.connect_direct().await.map_err(|error| {
        AppError::database_with_source(
            "Failed to connect to SQL for migrations.".to_string(),
            error,
        )
    })?;
    ensure_migration_table(&mut client).await?;

    let report = crate::embedded_migrations::migrations::runner()
        .set_migration_table_name(MIGRATION_TABLE_NAME)
        .run_async(&mut client)
        .await
        .map_err(|error| {
            AppError::database_with_source("Failed to apply SQL migrations.".to_string(), error)
        })?;

    Ok(report
        .applied_migrations()
        .iter()
        .map(|migration| migration.name().to_string())
        .collect())
}

async fn ensure_migration_table(client: &mut SqlClient) -> Result<(), AppError> {
    let stream = client
        .simple_query(ENSURE_MIGRATION_TABLE_QUERY)
        .await
        .map_err(|error| {
            AppError::database_with_source(
                format!(
                    "Failed to ensure the SQL migration history table at {MIGRATION_TABLE_FQ_NAME}."
                ),
                error,
            )
        })?;
    drain_query_stream(stream).await.map_err(|error| {
        AppError::database_with_source(
            format!(
                "Failed to initialize the SQL migration history table at {MIGRATION_TABLE_FQ_NAME}."
            ),
            error,
        )
    })
}

#[derive(Clone, Debug)]
struct SqlConnectionSettings {
    authentication: SqlAuthenticationMode,
    base_config: Config,
    database_name: String,
}

impl SqlConnectionSettings {
    fn parse(connection_string: &str) -> Result<Self, AppError> {
        let fields = parse_connection_fields(connection_string)?;
        let authentication = parse_authentication_mode(&fields)?;
        let database_name = read_required_field(&fields, &["database", "initialcatalog"])?;

        if authentication == SqlAuthenticationMode::SqlPassword {
            let user_id = read_optional_field(&fields, &["userid", "uid", "user", "username"]);
            let password = read_optional_field(&fields, &["password", "pwd"]);

            if user_id.is_none() || password.is_none() {
                return Err(AppError::config(
                    "SQL authentication requires both User ID and Password.".to_string(),
                ));
            }
        }

        let base_config = Config::from_ado_string(connection_string).map_err(|error| {
            AppError::config_with_source(
                "Failed to parse SQL connection string.".to_string(),
                error,
            )
        })?;

        Ok(Self {
            authentication,
            base_config,
            database_name,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SqlAuthenticationMode {
    ActiveDirectoryDefault { client_id: Option<String> },
    ManagedIdentity { client_id: Option<String> },
    SqlPassword,
}

struct SqlDefaultCredential {
    cached_source: Mutex<Option<CachedCredentialSource>>,
    client_id: Option<String>,
    #[cfg(test)]
    test_sources: Option<Vec<TestCredentialSource>>,
}

impl std::fmt::Debug for SqlDefaultCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqlDefaultCredential")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl SqlDefaultCredential {
    fn new(client_id: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            cached_source: Mutex::new(None),
            client_id,
            #[cfg(test)]
            test_sources: None,
        })
    }

    #[cfg(test)]
    fn new_with_test_sources(sources: Vec<TestCredentialSource>) -> Arc<Self> {
        Arc::new(Self {
            cached_source: Mutex::new(None),
            client_id: None,
            test_sources: Some(sources),
        })
    }
}

#[async_trait]
impl TokenCredential for SqlDefaultCredential {
    async fn get_token(
        &self,
        scopes: &[&str],
        options: Option<TokenRequestOptions<'_>>,
    ) -> azure_core::Result<AccessToken> {
        #[cfg(test)]
        if let Some(sources) = self.test_sources.clone() {
            let cached_source = self
                .cached_source
                .lock()
                .expect("default credential lock poisoned")
                .clone();

            if let Some(source) = cached_source {
                return source.credential.get_token(scopes, options).await;
            }

            let mut errors = Vec::new();

            for source in sources {
                match source.credential.get_token(scopes, options.clone()).await {
                    Ok(token) => {
                        *self
                            .cached_source
                            .lock()
                            .expect("default credential lock poisoned") =
                            Some(CachedCredentialSource {
                                credential: source.credential,
                            });
                        return Ok(token);
                    }
                    Err(error) => errors.push(format_credential_attempt_error(source.name, &error)),
                }
            }

            return Err(AzureError::with_message(
                AzureErrorKind::Credential,
                format!(
                    "ActiveDirectoryDefault authentication failed after trying: {}",
                    errors.join(" | ")
                ),
            ));
        }

        let cached_source = self
            .cached_source
            .lock()
            .expect("default credential lock poisoned")
            .clone();

        if let Some(source) = cached_source {
            return source.credential.get_token(scopes, options).await;
        }

        let mut errors = Vec::new();

        for source_kind in SqlDefaultCredentialSource::ordered_sources() {
            let source = match source_kind.build(self.client_id.clone()) {
                Ok(source) => source,
                Err(error) => {
                    errors.push(format_credential_attempt_error(source_kind.name(), &error));
                    continue;
                }
            };

            match source.credential.get_token(scopes, options.clone()).await {
                Ok(token) => {
                    *self
                        .cached_source
                        .lock()
                        .expect("default credential lock poisoned") = Some(source);
                    return Ok(token);
                }
                Err(error) => {
                    errors.push(format_credential_attempt_error(source_kind.name(), &error));
                }
            }
        }

        Err(AzureError::with_message(
            AzureErrorKind::Credential,
            format!(
                "ActiveDirectoryDefault authentication failed after trying: {}",
                errors.join(" | ")
            ),
        ))
    }
}

#[derive(Clone)]
struct CachedCredentialSource {
    credential: Arc<SqlTokenCredential>,
}

#[cfg(test)]
#[derive(Clone)]
struct TestCredentialSource {
    credential: Arc<SqlTokenCredential>,
    name: &'static str,
}

#[derive(Clone, Copy, Debug)]
enum SqlDefaultCredentialSource {
    WorkloadIdentity,
    ManagedIdentity,
    DeveloperTools,
}

impl SqlDefaultCredentialSource {
    const fn ordered_sources() -> [Self; 3] {
        [
            Self::WorkloadIdentity,
            Self::ManagedIdentity,
            Self::DeveloperTools,
        ]
    }

    const fn name(self) -> &'static str {
        match self {
            Self::WorkloadIdentity => "WorkloadIdentityCredential",
            Self::ManagedIdentity => "ManagedIdentityCredential",
            Self::DeveloperTools => "DeveloperToolsCredential",
        }
    }

    fn build(self, client_id: Option<String>) -> azure_core::Result<CachedCredentialSource> {
        let credential = match self {
            Self::WorkloadIdentity => create_workload_identity_credential(client_id)?,
            Self::ManagedIdentity => create_managed_identity_credential(client_id)?,
            Self::DeveloperTools => create_developer_tools_credential()?,
        };

        Ok(CachedCredentialSource { credential })
    }
}

fn create_developer_tools_credential() -> azure_core::Result<Arc<SqlTokenCredential>> {
    Ok(DeveloperToolsCredential::new(None)?)
}

fn create_managed_identity_credential(
    client_id: Option<String>,
) -> azure_core::Result<Arc<SqlTokenCredential>> {
    let options = client_id.map(|client_id| ManagedIdentityCredentialOptions {
        user_assigned_id: Some(UserAssignedId::ClientId(client_id)),
        ..ManagedIdentityCredentialOptions::default()
    });

    Ok(ManagedIdentityCredential::new(options)?)
}

fn create_workload_identity_credential(
    client_id: Option<String>,
) -> azure_core::Result<Arc<SqlTokenCredential>> {
    let options = client_id.map(|client_id| WorkloadIdentityCredentialOptions {
        client_id: Some(client_id),
        ..WorkloadIdentityCredentialOptions::default()
    });

    Ok(WorkloadIdentityCredential::new(options)?)
}

fn format_credential_attempt_error(source_name: &str, error: &AzureError) -> String {
    let mut messages = Vec::new();
    let mut current: Option<&dyn std::error::Error> = Some(error);

    while let Some(value) = current.take() {
        messages.push(value.to_string());
        current = value.source();
    }

    format!("{source_name} ({})", messages.join(" - "))
}

enum SqlConnectionManager {
    Password(ConnectionManager),
    Aad(AadConnectionManager),
}

impl SqlConnectionManager {
    fn from_settings(settings: &SqlConnectionSettings) -> Result<Self, AppError> {
        match &settings.authentication {
            SqlAuthenticationMode::SqlPassword => Ok(Self::Password(ConnectionManager::new(
                settings.base_config.clone(),
            ))),
            SqlAuthenticationMode::ActiveDirectoryDefault { client_id } => {
                let credential: Arc<SqlTokenCredential> =
                    SqlDefaultCredential::new(client_id.clone());
                Ok(Self::Aad(AadConnectionManager::new(
                    settings.base_config.clone(),
                    credential,
                )))
            }
            SqlAuthenticationMode::ManagedIdentity { client_id } => {
                let credential =
                    create_managed_identity_credential(client_id.clone()).map_err(|error| {
                        AppError::config_with_source(
                            "Failed to initialize Azure managed identity credentials.".to_string(),
                            error,
                        )
                    })?;

                Ok(Self::Aad(AadConnectionManager::new(
                    settings.base_config.clone(),
                    credential,
                )))
            }
        }
    }

    async fn connect_direct(&self) -> Result<SqlClient, SqlConnectionError> {
        match self {
            Self::Password(manager) => ManageConnection::connect(manager).await.map_err(Into::into),
            Self::Aad(manager) => manager.connect().await,
        }
    }
}

impl ManageConnection for SqlConnectionManager {
    type Connection = SqlClient;
    type Error = SqlConnectionError;

    async fn connect(&self) -> Result<Self::Connection, Self::Error> {
        self.connect_direct().await
    }

    async fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        match self {
            Self::Password(manager) => ManageConnection::is_valid(manager, conn)
                .await
                .map_err(Into::into),
            Self::Aad(manager) => manager.is_valid(conn).await,
        }
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        match self {
            Self::Password(manager) => ManageConnection::has_broken(manager, conn),
            Self::Aad(manager) => manager.has_broken(conn),
        }
    }
}

struct AadConnectionManager {
    base_config: Config,
    credential: Arc<SqlTokenCredential>,
}

impl AadConnectionManager {
    fn new(base_config: Config, credential: Arc<SqlTokenCredential>) -> Self {
        Self {
            base_config,
            credential,
        }
    }

    async fn connect(&self) -> Result<SqlClient, SqlConnectionError> {
        let mut config = self.base_config.clone();
        let token = self
            .credential
            .get_token(&[SQL_TOKEN_SCOPE], None)
            .await
            .map_err(SqlConnectionError::Azure)?;

        config.authentication(AuthMethod::aad_token(token.token.secret()));
        connect_with_config(config).await
    }

    async fn is_valid(&self, conn: &mut SqlClient) -> Result<(), SqlConnectionError> {
        let stream = conn.simple_query("SELECT 1").await?;
        drain_query_stream(stream).await?;
        Ok(())
    }

    fn has_broken(&self, _conn: &mut SqlClient) -> bool {
        false
    }
}

#[derive(Debug, Error)]
enum SqlConnectionError {
    #[error(transparent)]
    Azure(#[from] azure_core::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Pool(#[from] bb8_tiberius::Error),
    #[error(transparent)]
    Tiberius(#[from] tiberius::error::Error),
}

async fn connect_with_config(config: Config) -> Result<SqlClient, SqlConnectionError> {
    let tcp = TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;

    match SqlClient::connect(config.clone(), tcp.compat_write()).await {
        Ok(client) => Ok(client),
        Err(tiberius::error::Error::Routing { host, port }) => {
            let mut redirected = config;
            redirected.host(host);
            redirected.port(port);

            let tcp = TcpStream::connect(redirected.get_addr()).await?;
            tcp.set_nodelay(true)?;

            Ok(SqlClient::connect(redirected, tcp.compat_write()).await?)
        }
        Err(error) => Err(error.into()),
    }
}

async fn ensure_database_exists(settings: &SqlConnectionSettings) -> Result<(), AppError> {
    if settings.authentication != SqlAuthenticationMode::SqlPassword {
        return Ok(());
    }

    let mut config = settings.base_config.clone();
    config.database("master");

    let manager = SqlConnectionManager::Password(ConnectionManager::new(config));
    let mut client = manager.connect_direct().await.map_err(|error| {
        AppError::database_with_source(
            "Failed to connect to SQL while ensuring the database exists.".to_string(),
            error,
        )
    })?;

    let create_statement = format!(
        "IF DB_ID(N'{}') IS NULL BEGIN EXEC(N'CREATE DATABASE {}'); END",
        settings.database_name.replace('\'', "''"),
        quote_sql_identifier(&settings.database_name),
    );

    let stream = client
        .simple_query(create_statement)
        .await
        .map_err(|error| {
            AppError::database_with_source(
                "Failed to ensure the SQL database exists.".to_string(),
                error,
            )
        })?;
    drain_query_stream(stream).await.map_err(|error| {
        AppError::database_with_source("Failed to initialize the SQL database.".to_string(), error)
    })?;

    Ok(())
}

async fn drain_query_stream(
    stream: tiberius::QueryStream<'_>,
) -> Result<(), tiberius::error::Error> {
    stream.into_results().await.map(|_| ())
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
        .flat_map(|character| character.to_lowercase())
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

fn read_optional_field(fields: &BTreeMap<String, String>, names: &[&str]) -> Option<String> {
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

fn quote_sql_identifier(value: &str) -> String {
    format!("[{}]", value.replace(']', "]]"))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, SystemTime},
    };

    use azure_core::credentials::AccessToken;

    use super::*;

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

    #[derive(Debug)]
    struct MockCredential {
        call_count: AtomicUsize,
        error_message: Option<&'static str>,
        id: &'static str,
    }

    impl MockCredential {
        fn failing(id: &'static str, error_message: &'static str) -> Arc<Self> {
            Arc::new(Self {
                call_count: AtomicUsize::new(0),
                error_message: Some(error_message),
                id,
            })
        }

        fn succeeding(id: &'static str) -> Arc<Self> {
            Arc::new(Self {
                call_count: AtomicUsize::new(0),
                error_message: None,
                id,
            })
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl TokenCredential for MockCredential {
        async fn get_token(
            &self,
            _scopes: &[&str],
            _options: Option<TokenRequestOptions<'_>>,
        ) -> azure_core::Result<AccessToken> {
            self.call_count.fetch_add(1, Ordering::SeqCst);

            match self.error_message {
                Some(message) => Err(AzureError::with_message(
                    AzureErrorKind::Credential,
                    message.to_string(),
                )),
                None => Ok(AccessToken {
                    token: self.id.to_string().into(),
                    expires_on: (SystemTime::now() + Duration::from_secs(3600)).into(),
                }),
            }
        }
    }

    #[tokio::test]
    async fn default_credential_chain_uses_first_successful_source_and_caches_it() {
        let workload = MockCredential::failing("workload", "workload unavailable");
        let managed = MockCredential::failing("managed", "managed identity unavailable");
        let developer_tools = MockCredential::succeeding("developer-tools");
        let credential = SqlDefaultCredential::new_with_test_sources(vec![
            TestCredentialSource {
                credential: workload.clone(),
                name: "WorkloadIdentityCredential",
            },
            TestCredentialSource {
                credential: managed.clone(),
                name: "ManagedIdentityCredential",
            },
            TestCredentialSource {
                credential: developer_tools.clone(),
                name: "DeveloperToolsCredential",
            },
        ]);

        let first_token = credential
            .get_token(&[SQL_TOKEN_SCOPE], None)
            .await
            .unwrap();
        let second_token = credential
            .get_token(&[SQL_TOKEN_SCOPE], None)
            .await
            .unwrap();

        assert_eq!(first_token.token.secret(), "developer-tools");
        assert_eq!(second_token.token.secret(), "developer-tools");
        assert_eq!(workload.call_count(), 1);
        assert_eq!(managed.call_count(), 1);
        assert_eq!(developer_tools.call_count(), 2);
    }

    #[tokio::test]
    async fn default_credential_chain_reports_all_attempted_sources() {
        let credential = SqlDefaultCredential::new_with_test_sources(vec![
            TestCredentialSource {
                credential: MockCredential::failing("workload", "workload unavailable"),
                name: "WorkloadIdentityCredential",
            },
            TestCredentialSource {
                credential: MockCredential::failing("managed", "managed identity unavailable"),
                name: "ManagedIdentityCredential",
            },
            TestCredentialSource {
                credential: MockCredential::failing(
                    "developer-tools",
                    "developer tools unavailable",
                ),
                name: "DeveloperToolsCredential",
            },
        ]);

        let error = credential
            .get_token(&[SQL_TOKEN_SCOPE], None)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("WorkloadIdentityCredential"));
        assert!(error.contains("ManagedIdentityCredential"));
        assert!(error.contains("DeveloperToolsCredential"));
    }

    #[tokio::test]
    async fn ping_reuses_pooled_connections_when_local_sql_is_available() {
        let Some(connection_string) = local_sql_connection_string() else {
            return;
        };

        let settings = SqlConnectionSettings::parse(&connection_string).unwrap();
        ensure_database_exists(&settings).await.unwrap();

        let pool = Pool::builder()
            .max_size(1)
            .build(SqlConnectionManager::from_settings(&settings).unwrap())
            .await
            .unwrap();
        let database = Database { pool };

        database.ping().await.unwrap();
        database.ping().await.unwrap();
    }

    #[tokio::test]
    async fn migrations_can_run_multiple_times_when_local_sql_is_available() {
        let Some(connection_string) = local_sql_connection_string() else {
            return;
        };

        run_migrations(&connection_string).await.unwrap();
        run_migrations(&connection_string).await.unwrap();
    }

    fn local_sql_connection_string() -> Option<String> {
        let connection_string = std::env::var(crate::config::SQL_CONNECTION_STRING_NAME).ok()?;
        let normalized = connection_string.to_ascii_lowercase();

        if normalized.contains("127.0.0.1") || normalized.contains("localhost") {
            Some(connection_string)
        } else {
            None
        }
    }
}
