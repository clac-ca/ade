use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    routing::post,
};

use crate::{
    error::AppError,
    runs::{CreateUploadRequest, CreateUploadResponse, RunService},
    scope::Scope,
};

pub fn router() -> Router<crate::api::AppState> {
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
    let request = request
        .map(|Json(value)| value)
        .map_err(|error| AppError::request(error.body_text()))?;
    match run_service.create_upload(&scope, request).await {
        Ok(response) => Ok(Json(response)),
        Err(error) => {
            tracing::error!(
                workspace_id = %scope.workspace_id,
                config_version_id = %scope.config_version_id,
                error = ?error,
                "Failed to create upload access."
            );
            Err(error)
        }
    }
}
