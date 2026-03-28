use std::{convert::Infallible, sync::Arc};

use async_stream::stream;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{error::AppError, runtime::RuntimeService, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/session/ensure", post(ensure_session))
        .route("/session/reset", post(reset_session))
        .route("/events", get(events))
        .route("/files", post(upload_file))
        .route("/files/{*path}", get(download_file))
        .route("/terminal/open", post(terminal_open))
        .route("/terminal/input", post(terminal_input))
        .route("/terminal/resize", post(terminal_resize))
        .route("/terminal/close", post(terminal_close))
        .route("/run/start", post(run_start))
        .route("/run/cancel", post(run_cancel))
}

#[derive(Serialize)]
struct EnsureSessionResponse {
    status: crate::runtime::RuntimeStatus,
}

#[derive(Serialize)]
struct ResetSessionResponse {
    reset: bool,
}

#[derive(Serialize)]
struct RpcResponse {
    result: Value,
}

#[derive(Deserialize)]
struct UploadFileQuery {
    path: String,
}

#[derive(Deserialize)]
struct TerminalOpenRequest {
    rows: Option<u16>,
    cols: Option<u16>,
}

#[derive(Deserialize)]
struct TerminalInputRequest {
    data: String,
}

#[derive(Deserialize)]
struct TerminalResizeRequest {
    rows: u16,
    cols: u16,
}

#[derive(Deserialize)]
struct RunStartRequest {
    #[serde(rename = "inputPath")]
    input_path: String,
    #[serde(rename = "outputDir")]
    output_dir: Option<String>,
}

async fn ensure_session(
    State(state): State<AppState>,
) -> Result<Json<EnsureSessionResponse>, AppError> {
    let service = runtime_service(&state)?;
    let status = service.ensure_ready().await?;
    Ok(Json(EnsureSessionResponse { status }))
}

async fn reset_session(
    State(state): State<AppState>,
) -> Result<Json<ResetSessionResponse>, AppError> {
    let service = runtime_service(&state)?;
    service.reset_session().await?;
    Ok(Json(ResetSessionResponse { reset: true }))
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, AppError> {
    let service = runtime_service(&state)?;
    let _ = service.ensure_ready().await?;
    let mut after = parse_last_event_id(&headers);
    let keep_alive = KeepAlive::new().interval(std::time::Duration::from_secs(10));

    let event_stream = stream! {
        loop {
            match service.poll_events(after, service.default_event_wait_ms()).await {
                Ok(batch) => {
                    if batch.needs_resync {
                        after = batch.status.latest_seq;
                        let mut event = Event::default().event("runtime.resync_required");
                        if batch.status.latest_seq > 0 {
                            event = event.id(batch.status.latest_seq.to_string());
                        }
                        let payload = json!({ "status": batch.status });
                        yield Ok(event.json_data(payload).expect("valid SSE JSON payload"));
                        continue;
                    }

                    if batch.events.is_empty() {
                        continue;
                    }

                    for runtime_event in batch.events {
                        after = runtime_event.seq;
                        let event = Event::default()
                            .id(runtime_event.seq.to_string())
                            .event(runtime_event.event_type.clone())
                            .json_data(runtime_event)
                            .expect("valid SSE JSON payload");
                        yield Ok(event);
                    }
                }
                Err(error) => {
                    let event = Event::default()
                        .event("runtime.error")
                        .json_data(json!({ "message": error.to_string() }))
                        .expect("valid SSE JSON payload");
                    yield Ok(event);
                    break;
                }
            }
        }
    };

    Ok(Sse::new(event_stream).keep_alive(keep_alive))
}

async fn upload_file(
    State(state): State<AppState>,
    Query(query): Query<UploadFileQuery>,
    body: axum::body::Bytes,
) -> Result<Json<crate::runtime::UploadedFile>, AppError> {
    let service = runtime_service(&state)?;
    let uploaded = service.upload_file(&query.path, body.to_vec()).await?;
    Ok(Json(uploaded))
}

async fn download_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Response, AppError> {
    let service = runtime_service(&state)?;
    let content = service.download_file(&path).await?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from(content),
    )
        .into_response())
}

async fn terminal_open(
    State(state): State<AppState>,
    Json(request): Json<TerminalOpenRequest>,
) -> Result<Json<RpcResponse>, AppError> {
    rpc_response(
        runtime_service(&state)?,
        "terminal.open",
        json!({
            "rows": request.rows.unwrap_or(24),
            "cols": request.cols.unwrap_or(80),
        }),
    )
    .await
}

async fn terminal_input(
    State(state): State<AppState>,
    Json(request): Json<TerminalInputRequest>,
) -> Result<Json<RpcResponse>, AppError> {
    rpc_response(
        runtime_service(&state)?,
        "terminal.input",
        json!({ "data": request.data }),
    )
    .await
}

async fn terminal_resize(
    State(state): State<AppState>,
    Json(request): Json<TerminalResizeRequest>,
) -> Result<Json<RpcResponse>, AppError> {
    rpc_response(
        runtime_service(&state)?,
        "terminal.resize",
        json!({ "rows": request.rows, "cols": request.cols }),
    )
    .await
}

async fn terminal_close(State(state): State<AppState>) -> Result<Json<RpcResponse>, AppError> {
    rpc_response(runtime_service(&state)?, "terminal.close", json!({})).await
}

async fn run_start(
    State(state): State<AppState>,
    Json(request): Json<RunStartRequest>,
) -> Result<Json<RpcResponse>, AppError> {
    rpc_response(
        runtime_service(&state)?,
        "run.start",
        json!({
            "inputPath": request.input_path,
            "outputDir": request.output_dir.unwrap_or_else(|| "outputs".to_string()),
        }),
    )
    .await
}

async fn run_cancel(State(state): State<AppState>) -> Result<Json<RpcResponse>, AppError> {
    rpc_response(runtime_service(&state)?, "run.cancel", json!({})).await
}

async fn rpc_response(
    service: Arc<RuntimeService>,
    method: &str,
    params: Value,
) -> Result<Json<RpcResponse>, AppError> {
    let result = service.rpc(method, params).await?;
    Ok(Json(RpcResponse { result }))
}

fn runtime_service(state: &AppState) -> Result<Arc<RuntimeService>, AppError> {
    state
        .runtime
        .clone()
        .ok_or_else(|| AppError::unavailable("Hosted ADE runtime is not configured.".to_string()))
}

fn parse_last_event_id(headers: &HeaderMap) -> u64 {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}
