use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Path, State, rejection::JsonRejection},
    http::header,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;

use crate::{
    error::AppError,
    runs::RunService,
    session::{ExecuteCommandRequest, Scope},
    session::{SessionFile, SessionService},
};

pub fn router() -> Router<crate::router::AppState> {
    Router::new()
        .route("/executions", post(execute_command))
        .route("/files", post(upload_file).get(list_files))
        .route("/files/{*path}", axum::routing::get(download_file))
}

#[utoipa::path(
    post,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/executions",
    tag = "session",
    params(Scope),
    request_body = ExecuteCommandRequest,
    responses(
        (status = 200, description = "Command result", body = crate::session::ExecuteCommandResponse),
        (status = 400, description = "Invalid request", body = crate::error::ErrorResponse),
        (status = 404, description = "Scope not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn execute_command(
    State(session_service): State<Arc<SessionService>>,
    Path(scope): Path<Scope>,
    request: Result<Json<ExecuteCommandRequest>, JsonRejection>,
) -> Result<Json<crate::session::ExecuteCommandResponse>, AppError> {
    let request = parse_json(request)?;
    Ok(Json(
        session_service
            .execute_command(&scope, &request.shell_command, request.timeout_in_seconds)
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/files",
    tag = "session",
    params(Scope),
    request_body(
        content = crate::api_docs::UploadFileBody,
        content_type = "multipart/form-data"
    ),
    responses(
        (status = 200, description = "Uploaded file metadata", body = SessionFile),
        (status = 400, description = "Invalid upload", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn upload_file(
    State(run_service): State<Arc<RunService>>,
    Path(scope): Path<Scope>,
    multipart: Multipart,
) -> Result<Json<SessionFile>, AppError> {
    let (filename, content_type, content) = read_uploaded_file(multipart).await?;
    Ok(Json(
        run_service
            .upload_file(&scope, filename, content_type, content)
            .await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/files",
    tag = "session",
    params(Scope),
    responses(
        (status = 200, description = "Session files", body = [SessionFile]),
        (status = 404, description = "Scope not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn list_files(
    State(run_service): State<Arc<RunService>>,
    Path(scope): Path<Scope>,
) -> Result<Json<Vec<SessionFile>>, AppError> {
    Ok(Json(run_service.list_files(&scope).await?))
}

#[utoipa::path(
    get,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/files/{path}/content",
    tag = "session",
    params(
        Scope,
        ("path" = String, Path, description = "Session-relative file path")
    ),
    responses(
        (status = 200, description = "Raw file bytes"),
        (status = 404, description = "File not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn download_file(
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<ContentFilePath>,
) -> Result<Response, AppError> {
    let (scope, filename) = path.into_parts()?;
    bytes_response(run_service.download_file(&scope, &filename).await)
}

#[derive(Deserialize)]
pub(crate) struct ContentFilePath {
    path: String,
    #[serde(rename = "workspaceId")]
    workspace_id: String,
    #[serde(rename = "configVersionId")]
    config_version_id: String,
}

impl ContentFilePath {
    fn into_parts(self) -> Result<(Scope, String), AppError> {
        let filename = self
            .path
            .strip_suffix("/content")
            .ok_or_else(|| AppError::not_found("Route not found"))?
            .to_string();
        Ok((
            Scope {
                workspace_id: self.workspace_id,
                config_version_id: self.config_version_id,
            },
            filename,
        ))
    }
}

fn parse_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, AppError> {
    request
        .map(|Json(value)| value)
        .map_err(|error| AppError::request(error.body_text()))
}

async fn read_uploaded_file(
    mut multipart: Multipart,
) -> Result<(String, Option<String>, Vec<u8>), AppError> {
    while let Some(field) = multipart.next_field().await.map_err(|error| {
        AppError::request(format!("Failed to read the multipart upload body: {error}"))
    })? {
        if field.name() != Some("file") {
            continue;
        }

        let filename = field.file_name().map(ToOwned::to_owned).ok_or_else(|| {
            AppError::request("Uploaded file must include a filename.".to_string())
        })?;
        let content_type = field.content_type().map(ToOwned::to_owned);
        let content = field.bytes().await.map_err(|error| {
            AppError::request(format!("Failed to read the uploaded file content: {error}"))
        })?;
        return Ok((filename, content_type, content.to_vec()));
    }

    Err(AppError::request(
        "Multipart upload must include a file field.".to_string(),
    ))
}

fn bytes_response(response: Result<(String, Vec<u8>), AppError>) -> Result<Response, AppError> {
    let (content_type, body) = response?;
    Ok(([(header::CONTENT_TYPE, content_type)], Body::from(body)).into_response())
}
