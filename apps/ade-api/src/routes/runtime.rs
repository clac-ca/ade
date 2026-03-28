use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::Value;

use crate::{
    error::AppError,
    runtime::{BytesProxyResponse, ExecutionRequest, JsonProxyResponse, RuntimeService},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/code/execute", post(executions))
        .route("/files/upload", post(upload_file))
        .route("/files", get(list_files))
        .route("/files/content/{filename}", get(download_file))
        .route("/.management/stopSession", post(stop_session))
        .route("/mcp", post(mcp))
}

async fn executions(
    State(state): State<AppState>,
    Json(request): Json<ExecutionRequest>,
) -> Result<Response, AppError> {
    json_proxy_response(runtime_service(&state)?.execute(request).await)
}

async fn upload_file(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut uploaded_file = None;

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
        uploaded_file = Some((filename, content_type, content.to_vec()));
        break;
    }

    let (filename, content_type, content) = uploaded_file.ok_or_else(|| {
        AppError::request("Multipart upload must include a file field.".to_string())
    })?;

    json_proxy_response(
        runtime_service(&state)?
            .upload_file(filename, content_type, content)
            .await,
    )
}

async fn list_files(State(state): State<AppState>) -> Result<Response, AppError> {
    json_proxy_response(runtime_service(&state)?.list_files().await)
}

async fn download_file(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> Result<Response, AppError> {
    bytes_proxy_response(runtime_service(&state)?.download_file(&filename).await)
}

async fn stop_session(State(state): State<AppState>) -> Result<Response, AppError> {
    json_proxy_response(runtime_service(&state)?.stop_session().await)
}

async fn mcp(
    State(state): State<AppState>,
    Json(request): Json<Value>,
) -> Result<Response, AppError> {
    json_proxy_response(runtime_service(&state)?.mcp(request).await)
}

fn json_proxy_response(
    response: Result<JsonProxyResponse, AppError>,
) -> Result<Response, AppError> {
    let JsonProxyResponse { body, headers } = response?;
    let mut response = (StatusCode::OK, Json(body)).into_response();
    apply_forwarded_headers(response.headers_mut(), headers)?;
    Ok(response)
}

fn bytes_proxy_response(
    response: Result<BytesProxyResponse, AppError>,
) -> Result<Response, AppError> {
    let BytesProxyResponse {
        body,
        content_type,
        headers,
    } = response?;
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type)],
        Body::from(body),
    )
        .into_response();
    apply_forwarded_headers(response.headers_mut(), headers)?;
    Ok(response)
}

fn apply_forwarded_headers(
    target: &mut HeaderMap,
    headers: Vec<(String, String)>,
) -> Result<(), AppError> {
    for (name, value) in headers {
        let header_name = header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            AppError::internal_with_source(
                format!("Failed to forward runtime header '{name}'."),
                error,
            )
        })?;
        let header_value = header::HeaderValue::from_str(&value).map_err(|error| {
            AppError::internal_with_source(
                format!("Failed to forward runtime header '{name}'."),
                error,
            )
        })?;
        target.insert(header_name, header_value);
    }
    Ok(())
}

fn runtime_service(state: &AppState) -> Result<Arc<RuntimeService>, AppError> {
    state
        .runtime
        .clone()
        .ok_or_else(|| AppError::unavailable("Hosted ADE runtime is not configured.".to_string()))
}
