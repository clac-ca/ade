use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::JsonRejection,
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    error::AppError,
    runs::{AsyncRunResponse, RunDetailResponse, RunService},
    session::{CreateRunRequest, RunResponse, Scope},
};

pub fn workspace_router() -> Router<crate::router::AppState> {
    Router::new()
        .route("/runs", post(create_run))
        .route("/runs/{runId}", get(get_run))
        .route("/runs/{runId}/events", get(connect_run_events))
        .route("/runs/{runId}/cancel", post(cancel_run))
}

pub fn internal_router() -> Router<crate::router::AppState> {
    Router::new().route("/run-bridges/{bridgeId}", get(connect_internal_bridge))
}

#[utoipa::path(
    post,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs",
    tag = "runs",
    params(Scope),
    request_body = CreateRunRequest,
    responses(
        (status = 200, description = "ADE run result", body = RunResponse),
        (status = 202, description = "Accepted async run", body = AsyncRunResponse),
        (status = 400, description = "Invalid request", body = crate::error::ErrorResponse),
        (status = 404, description = "Scope or input file not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
async fn create_run(
    State(run_service): State<Arc<RunService>>,
    Path(scope): Path<Scope>,
    headers: HeaderMap,
    request: Result<Json<CreateRunRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let request = parse_json(request)?;

    if prefers_async(&headers) {
        let response = run_service.create_async_run(scope.clone(), request).await?;
        let location = format!(
            "/api/workspaces/{}/configs/{}/runs/{}",
            scope.workspace_id, scope.config_version_id, response.run_id
        );
        let mut http_response = (StatusCode::ACCEPTED, Json(response)).into_response();
        http_response.headers_mut().insert(
            header::LOCATION,
            HeaderValue::from_str(&location).map_err(|error| {
                AppError::internal_with_source(
                    "Failed to encode the async run location header.",
                    error,
                )
            })?,
        );
        http_response.headers_mut().insert(
            header::HeaderName::from_static("preference-applied"),
            HeaderValue::from_static("respond-async"),
        );
        return Ok(http_response);
    }

    Ok(Json(run_service.create_sync_run(scope, request).await?).into_response())
}

#[utoipa::path(
    get,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}",
    tag = "runs",
    params(
        Scope,
        ("runId" = String, Path, description = "Run identifier")
    ),
    responses(
        (status = 200, description = "Run detail", body = RunDetailResponse),
        (status = 404, description = "Run not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
async fn get_run(
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<RunPath>,
) -> Result<Json<RunDetailResponse>, AppError> {
    Ok(Json(
        run_service
            .get_run_detail(&path.scope(), &path.run_id)
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/cancel",
    tag = "runs",
    params(
        Scope,
        ("runId" = String, Path, description = "Run identifier")
    ),
    responses(
        (status = 204, description = "Run cancelled"),
        (status = 404, description = "Run not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
async fn cancel_run(
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<RunPath>,
) -> Result<StatusCode, AppError> {
    run_service.cancel_run(&path.scope(), &path.run_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn connect_run_events(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<RunPath>,
) -> Result<Response, AppError> {
    let ws = ws.map_err(map_websocket_rejection)?;
    let scope = path.scope();
    let run_id = path.run_id;

    Ok(ws
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            run_service.stream_run_events(scope, socket, run_id).await;
        }))
}

async fn connect_internal_bridge(
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<BridgePath>,
    Query(query): Query<BridgeQuery>,
) -> Result<Response, AppError> {
    let ws = ws.map_err(map_websocket_rejection)?;
    let bridge_tx = run_service.claim_bridge(&path.bridge_id, &query.token)?;

    Ok(ws
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            run_service.attach_bridge_socket(socket, bridge_tx).await;
        }))
}

#[derive(Deserialize)]
struct BridgePath {
    #[serde(rename = "bridgeId")]
    bridge_id: String,
}

#[derive(Deserialize)]
struct BridgeQuery {
    token: String,
}

#[derive(Deserialize)]
pub(crate) struct RunPath {
    #[serde(rename = "configVersionId")]
    config_version_id: String,
    #[serde(rename = "runId")]
    run_id: String,
    #[serde(rename = "workspaceId")]
    workspace_id: String,
}

impl RunPath {
    fn scope(&self) -> Scope {
        Scope {
            workspace_id: self.workspace_id.clone(),
            config_version_id: self.config_version_id.clone(),
        }
    }
}

fn parse_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, AppError> {
    request
        .map(|Json(value)| value)
        .map_err(|error| AppError::request(error.body_text()))
}

fn prefers_async(headers: &HeaderMap) -> bool {
    headers
        .get("prefer")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|part| part.eq_ignore_ascii_case("respond-async"))
        })
}

fn map_websocket_rejection(error: WebSocketUpgradeRejection) -> AppError {
    AppError::request(error.to_string())
}
