use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;

use crate::{artifacts::LOCAL_ARTIFACT_TOKEN_HEADER, error::AppError, runs::RunService};

pub fn router() -> Router<crate::api::AppState> {
    Router::new().route("/artifacts/{*path}", get(download).put(upload))
}

async fn download(
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<InternalArtifactPath>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let token = headers
        .get(LOCAL_ARTIFACT_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            AppError::status(StatusCode::UNAUTHORIZED, "Missing artifact access token.")
        })?;
    let (content_type, body) = run_service
        .download_local_artifact(&path.path, token)
        .await?;
    Ok(([(header::CONTENT_TYPE, content_type)], body).into_response())
}

async fn upload(
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<InternalArtifactPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    let token = headers
        .get(LOCAL_ARTIFACT_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            AppError::status(StatusCode::UNAUTHORIZED, "Missing artifact access token.")
        })?;
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
