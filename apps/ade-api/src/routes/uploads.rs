use std::{error::Error as StdError, sync::Arc};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    artifacts::local_artifact_token_header,
    error::AppError,
    runs::{CreateUploadRequest, CreateUploadResponse, RunService},
    session::Scope,
};

pub fn workspace_router() -> Router<crate::router::AppState> {
    Router::new().route("/uploads", post(create_upload))
}

pub fn internal_router() -> Router<crate::router::AppState> {
    Router::new().route("/artifacts/{*path}", get(download_artifact).put(upload_artifact))
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

async fn download_artifact(
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<InternalArtifactPath>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let token = required_token(&headers)?;
    let (content_type, body) = run_service
        .download_local_artifact(&path.path, token)
        .await?;
    Ok(([(header::CONTENT_TYPE, content_type)], body).into_response())
}

async fn upload_artifact(
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<InternalArtifactPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    let token = required_token(&headers)?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    run_service
        .upload_local_artifact(&path.path, token, content_type, body.to_vec())
        .await?;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct InternalArtifactPath {
    path: String,
}

fn parse_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, AppError> {
    request
        .map(|Json(value)| value)
        .map_err(|error| AppError::request(error.body_text()))
}

fn required_token(headers: &HeaderMap) -> Result<&str, AppError> {
    headers
        .get(local_artifact_token_header())
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            AppError::status(StatusCode::UNAUTHORIZED, "Missing artifact access token.")
        })
}

fn error_sources(error: &AppError) -> String {
    let mut sources = Vec::new();
    let mut source = error.source();
    while let Some(current) = source {
        sources.push(current.to_string());
        source = current.source();
    }
    sources.join(" | ")
}
