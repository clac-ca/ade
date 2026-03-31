use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    routing::post,
};

use crate::{
    error::AppError,
    runs::{
        CreateUploadBatchRequest, CreateUploadBatchResponse, CreateUploadRequest,
        CreateUploadResponse, RunService,
    },
    scope::Scope,
};

pub fn router() -> Router<crate::api::AppState> {
    Router::new()
        .route("/uploads", post(create_upload))
        .route("/uploads/batches", post(create_upload_batch))
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
    Ok(Json(run_service.create_upload(&scope, request).await?))
}

#[utoipa::path(
    post,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/uploads/batches",
    tag = "uploads",
    params(Scope),
    request_body = CreateUploadBatchRequest,
    responses(
        (status = 200, description = "Bulk upload instructions", body = CreateUploadBatchResponse),
        (status = 400, description = "Invalid upload batch request", body = crate::error::ErrorResponse),
        (status = 404, description = "Scope not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
async fn create_upload_batch(
    State(run_service): State<Arc<RunService>>,
    Path(scope): Path<Scope>,
    request: Result<Json<CreateUploadBatchRequest>, JsonRejection>,
) -> Result<Json<CreateUploadBatchResponse>, AppError> {
    let request = request
        .map(|Json(value)| value)
        .map_err(|error| AppError::request(error.body_text()))?;
    Ok(Json(
        run_service.create_upload_batch(&scope, request).await?,
    ))
}
