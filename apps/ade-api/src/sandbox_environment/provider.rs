use crate::{
    config::EnvBag,
    error::AppError,
    session_pool::{SessionExecution, SessionFile, SessionOperationResult, SessionPoolClient},
};

/// Provider adapter for uploading files to, and executing commands within, the
/// Azure session-pool sandbox.
#[derive(Clone)]
pub(crate) struct SandboxProvider {
    session_pool_client: SessionPoolClient,
}

impl SandboxProvider {
    pub(crate) fn from_env(env: &EnvBag) -> Result<Self, AppError> {
        Ok(Self::new(SessionPoolClient::from_env(env)?))
    }

    pub(crate) fn new(session_pool_client: SessionPoolClient) -> Self {
        Self {
            session_pool_client,
        }
    }

    pub(crate) async fn execute(
        &self,
        sandbox_id: &str,
        shell_command: String,
        timeout_in_seconds: Option<u64>,
    ) -> Result<SessionOperationResult<SessionExecution>, AppError> {
        self.session_pool_client
            .execute(sandbox_id, shell_command, timeout_in_seconds)
            .await
    }

    pub(crate) async fn upload_file(
        &self,
        sandbox_id: &str,
        path: Option<&str>,
        filename: &str,
        content_type: Option<&str>,
        content: Vec<u8>,
    ) -> Result<SessionOperationResult<SessionFile>, AppError> {
        self.session_pool_client
            .upload_file(sandbox_id, path, filename, content_type, content)
            .await
    }

    pub(crate) async fn download_file(
        &self,
        sandbox_id: &str,
        path: Option<&str>,
        filename: &str,
    ) -> Result<SessionOperationResult<Vec<u8>>, AppError> {
        self.session_pool_client
            .download_file(sandbox_id, path, filename)
            .await
    }
}
