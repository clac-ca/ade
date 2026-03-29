use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Path, State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;

use crate::{
    error::AppError,
    session::{CreateRunRequest, ExecuteCommandRequest, RunResponse, Scope},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/executions", post(execute_command))
        .route("/files", post(upload_file).get(list_files))
        .route("/files/{*path}", axum::routing::get(download_file))
        .route("/runs", post(create_run))
}

async fn execute_command(
    State(state): State<AppState>,
    Path(scope): Path<Scope>,
    request: Result<Json<ExecuteCommandRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(request) = request.map_err(|error| AppError::request(error.body_text()))?;
    Ok((
        StatusCode::OK,
        Json(
            state
                .session_service
                .execute_command(&scope, &request.shell_command, request.timeout_in_seconds)
                .await?,
        ),
    )
        .into_response())
}

async fn upload_file(
    State(state): State<AppState>,
    Path(scope): Path<Scope>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let (filename, content_type, content) = read_uploaded_file(multipart).await?;
    Ok((
        StatusCode::OK,
        Json(
            state
                .session_service
                .upload_file(&scope, filename, content_type, content)
                .await?,
        ),
    )
        .into_response())
}

async fn list_files(
    State(state): State<AppState>,
    Path(scope): Path<Scope>,
) -> Result<Response, AppError> {
    Ok((
        StatusCode::OK,
        Json(state.session_service.list_files(&scope).await?),
    )
        .into_response())
}

async fn download_file(
    State(state): State<AppState>,
    Path(path): Path<ContentFilePath>,
) -> Result<Response, AppError> {
    let Some(filename) = path.filename() else {
        return Err(AppError::not_found("Route not found".to_string()));
    };
    bytes_response(
        state
            .session_service
            .download_file(&path.scope(), filename)
            .await,
    )
}

async fn create_run(
    State(state): State<AppState>,
    Path(scope): Path<Scope>,
    request: Result<Json<CreateRunRequest>, JsonRejection>,
) -> Result<Json<RunResponse>, AppError> {
    let Json(request) = request.map_err(|error| AppError::request(error.body_text()))?;
    Ok(Json(
        state
            .session_service
            .run(&scope, &request.input_path, request.timeout_in_seconds)
            .await?,
    ))
}

#[derive(Deserialize)]
struct ContentFilePath {
    path: String,
    #[serde(rename = "workspaceId")]
    workspace_id: String,
    #[serde(rename = "configVersionId")]
    config_version_id: String,
}

impl ContentFilePath {
    fn scope(&self) -> Scope {
        Scope {
            workspace_id: self.workspace_id.clone(),
            config_version_id: self.config_version_id.clone(),
        }
    }

    fn filename(&self) -> Option<&str> {
        self.path.strip_suffix("/content")
    }
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
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type)],
        Body::from(body),
    )
        .into_response())
}
