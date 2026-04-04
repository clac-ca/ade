use std::sync::Arc;

use azure_core::credentials::TokenCredential;
use azure_identity::{
    DeveloperToolsCredential, ManagedIdentityCredential, ManagedIdentityCredentialOptions,
    UserAssignedId,
};

use crate::{
    config::{EnvBag, read_optional_trimmed_string},
    error::AppError,
};

pub(crate) const AZURE_CLIENT_ID_ENV_NAME: &str = "AZURE_CLIENT_ID";
pub(crate) type AzureTokenCredential = dyn TokenCredential + Send + Sync;

pub(crate) fn read_azure_client_id(env: &EnvBag) -> Option<String> {
    read_optional_trimmed_string(env, AZURE_CLIENT_ID_ENV_NAME)
}

pub(crate) fn create_developer_tools_credential() -> azure_core::Result<Arc<AzureTokenCredential>> {
    Ok(DeveloperToolsCredential::new(None)?)
}

pub(crate) fn create_managed_identity_credential(
    client_id: Option<String>,
) -> azure_core::Result<Arc<AzureTokenCredential>> {
    let options = client_id.map(|client_id| ManagedIdentityCredentialOptions {
        user_assigned_id: Some(UserAssignedId::ClientId(client_id)),
        ..ManagedIdentityCredentialOptions::default()
    });

    Ok(ManagedIdentityCredential::new(options)?)
}

pub(crate) async fn access_token_with_developer_fallback(
    scope: &str,
    client_id: Option<String>,
    developer_credential_error: &str,
    access_token_error: &str,
) -> Result<String, AppError> {
    if let Ok(credential) = create_managed_identity_credential(client_id)
        && let Ok(token) = credential.get_token(&[scope], None).await
    {
        return Ok(token.token.secret().to_string());
    }

    let credential = create_developer_tools_credential().map_err(|error| {
        AppError::internal_with_source(developer_credential_error.to_string(), error)
    })?;
    credential
        .get_token(&[scope], None)
        .await
        .map(|token| token.token.secret().to_string())
        .map_err(|error| AppError::internal_with_source(access_token_error.to_string(), error))
}
