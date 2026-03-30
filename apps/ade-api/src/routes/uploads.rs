use std::error::Error as StdError;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    routing::post,
};

use crate::{
    error::AppError,
    runs::{CreateUploadRequest, CreateUploadResponse, RunService},
    session::Scope,
};

pub fn workspace_router() -> Router<crate::router::AppState> {
    Router::new().route("/uploads", post(create_upload))
}

#[utoipa::path(
    post,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/uploads",
    tag = "uploads",
    params(Scope),
    request_body = CreateUploadRequest,
    responses(
        (status = 200, description = "Upload instructions", body = CreateUploadResponse),
        (status = 400, description = "Invalid upload request", body = crate::error::ErrorResponse),
        (status = 404, description = "Scope not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
async fn create_upload(
    State(run_service): State<Arc<RunService>>,
    Path(scope): Path<Scope>,
    request: Result<Json<CreateUploadRequest>, JsonRejection>,
) -> Result<Json<CreateUploadResponse>, AppError> {
    let request = parse_json(request)?;
    match run_service.create_upload(&scope, request).await {
        Ok(response) => Ok(Json(response)),
        Err(error) => {
            tracing::error!(
                workspace_id = %scope.workspace_id,
                config_version_id = %scope.config_version_id,
                error = %error,
                error_details = ?error,
                error_sources = %error_sources(&error),
                "Failed to create upload access."
            );
            Err(error)
        }
    }
}

fn parse_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, AppError> {
    request
        .map(|Json(value)| value)
        .map_err(|error| AppError::request(error.body_text()))
}

fn error_sources(error: &AppError) -> String {
    let mut sources = Vec::new();
    let mut source = StdError::source(error);
    while let Some(current) = source {
        sources.push(current.to_string());
        source = StdError::source(current);
    }
    sources.join(" | ")
}
