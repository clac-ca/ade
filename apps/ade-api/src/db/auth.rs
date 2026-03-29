use std::sync::{Arc, Mutex};

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

pub(super) const SQL_TOKEN_SCOPE: &str = "https://database.windows.net/.default";
pub(super) type SqlTokenCredential = dyn TokenCredential + Send + Sync;

pub(super) struct SqlDefaultCredential {
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
    pub(super) fn new(client_id: Option<String>) -> Self {
        Self {
            cached_source: Mutex::new(None),
            client_id,
            #[cfg(test)]
            test_sources: None,
        }
    }

    #[cfg(test)]
    fn new_with_test_sources(sources: Vec<TestCredentialSource>) -> Self {
        Self {
            cached_source: Mutex::new(None),
            client_id: None,
            test_sources: Some(sources),
        }
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

pub(super) fn create_developer_tools_credential() -> azure_core::Result<Arc<SqlTokenCredential>> {
    Ok(DeveloperToolsCredential::new(None)?)
}

pub(super) fn create_managed_identity_credential(
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

pub(super) fn format_credential_attempt_error(source_name: &str, error: &AzureError) -> String {
    let mut messages = Vec::new();
    let mut current: Option<&dyn std::error::Error> = Some(error);

    while let Some(value) = current.take() {
        messages.push(value.to_string());
        current = value.source();
    }

    format!("{source_name} ({})", messages.join(" - "))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, SystemTime},
    };

    use azure_core::credentials::AccessToken;

    use super::*;

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
}
