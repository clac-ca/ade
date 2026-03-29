mod auth;
mod connection_string;

use std::sync::Arc;

use async_trait::async_trait;
use bb8::{ManageConnection, Pool};
use bb8_tiberius::ConnectionManager;
use thiserror::Error;
use tiberius::{AuthMethod, Row, ToSql};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

use crate::error::AppError;

use self::{
    auth::{
        SQL_TOKEN_SCOPE, SqlDefaultCredential, SqlTokenCredential,
        create_managed_identity_credential,
    },
    connection_string::{SqlAuthenticationMode, SqlConnectionSettings, quote_sql_identifier},
};

type SqlClient = bb8_tiberius::rt::Client;

#[async_trait]
pub trait DatabaseProbe: Send + Sync {
    async fn ping(&self) -> Result<(), AppError>;
    async fn close(&self) -> Result<(), AppError>;
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
                AppError::database_with_source("Failed to initialize SQL pool.", error)
            })?;

        Ok(Self { pool })
    }

    pub async fn execute(&self, query: &str, params: &[&dyn ToSql]) -> Result<(), AppError> {
        let mut connection = self.pool.get().await.map_err(|error| {
            AppError::database_with_source("Failed to acquire SQL connection.", error)
        })?;
        connection.execute(query, params).await.map_err(|error| {
            AppError::database_with_source("Failed to execute a SQL statement.", error)
        })?;
        Ok(())
    }

    pub async fn query_all(
        &self,
        query: &str,
        params: &[&dyn ToSql],
    ) -> Result<Vec<Row>, AppError> {
        let mut connection = self.pool.get().await.map_err(|error| {
            AppError::database_with_source("Failed to acquire SQL connection.", error)
        })?;
        let stream = connection.query(query, params).await.map_err(|error| {
            AppError::database_with_source("Failed to execute a SQL query.", error)
        })?;
        stream.into_first_result().await.map_err(|error| {
            AppError::database_with_source("Failed to read SQL query results.", error)
        })
    }

    pub async fn query_optional(
        &self,
        query: &str,
        params: &[&dyn ToSql],
    ) -> Result<Option<Row>, AppError> {
        let mut rows = self.query_all(query, params).await?;
        Ok(rows.drain(..).next())
    }
}

#[async_trait]
impl DatabaseProbe for Database {
    async fn ping(&self) -> Result<(), AppError> {
        let mut connection = self.pool.get().await.map_err(|error| {
            AppError::database_with_source("Failed to acquire SQL connection.", error)
        })?;

        let stream = connection
            .simple_query("SELECT 1 AS value")
            .await
            .map_err(|error| AppError::database_with_source("SQL ping failed.", error))?;
        drain_query_stream(stream)
            .await
            .map_err(|error| AppError::database_with_source("SQL ping failed.", error))?;

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
        AppError::database_with_source("Failed to connect to SQL for migrations.", error)
    })?;
    let report = crate::embedded_migrations::migrations::runner()
        .run_async(&mut client)
        .await
        .map_err(|error| {
            AppError::database_with_source("Failed to apply SQL migrations.", error)
        })?;

    Ok(report
        .applied_migrations()
        .iter()
        .map(|migration| migration.name().to_string())
        .collect())
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
                    Arc::new(SqlDefaultCredential::new(client_id.clone()));
                Ok(Self::Aad(AadConnectionManager::new(
                    settings.base_config.clone(),
                    credential,
                )))
            }
            SqlAuthenticationMode::ManagedIdentity { client_id } => {
                let credential =
                    create_managed_identity_credential(client_id.clone()).map_err(|error| {
                        AppError::config_with_source(
                            "Failed to initialize Azure managed identity credentials.",
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
            Self::Aad(_) => false,
        }
    }
}

struct AadConnectionManager {
    base_config: tiberius::Config,
    credential: Arc<SqlTokenCredential>,
}

impl AadConnectionManager {
    fn new(base_config: tiberius::Config, credential: Arc<SqlTokenCredential>) -> Self {
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

async fn connect_with_config(config: tiberius::Config) -> Result<SqlClient, SqlConnectionError> {
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
            "Failed to connect to SQL while ensuring the database exists.",
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
            AppError::database_with_source("Failed to ensure the SQL database exists.", error)
        })?;
    drain_query_stream(stream).await.map_err(|error| {
        AppError::database_with_source("Failed to initialize the SQL database.", error)
    })?;

    Ok(())
}

async fn drain_query_stream(
    stream: tiberius::QueryStream<'_>,
) -> Result<(), tiberius::error::Error> {
    stream.into_results().await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::DatabaseProbe;

    #[tokio::test]
    async fn ping_reuses_pooled_connections_when_local_sql_is_available() {
        let Some(connection_string) = local_sql_connection_string() else {
            return;
        };

        let settings =
            super::connection_string::SqlConnectionSettings::parse(&connection_string).unwrap();
        super::ensure_database_exists(&settings).await.unwrap();

        let pool = bb8::Pool::builder()
            .max_size(1)
            .build(super::SqlConnectionManager::from_settings(&settings).unwrap())
            .await
            .unwrap();
        let database = super::Database { pool };

        database.ping().await.unwrap();
        database.ping().await.unwrap();
    }

    #[tokio::test]
    async fn migrations_can_run_multiple_times_when_local_sql_is_available() {
        let Some(connection_string) = local_sql_connection_string() else {
            return;
        };

        super::run_migrations(&connection_string).await.unwrap();
        super::run_migrations(&connection_string).await.unwrap();
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
