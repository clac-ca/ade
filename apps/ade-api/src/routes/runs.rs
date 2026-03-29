use std::{convert::Infallible, sync::Arc, time::Duration};

use async_stream::stream;
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::JsonRejection,
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response, sse::Event, sse::KeepAlive, sse::Sse},
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    error::AppError,
    run_store::RunEvent,
    runs::{AsyncRunResponse, RunDetailResponse, RunService},
    session::{CreateRunRequest, Scope},
};

pub fn workspace_router() -> Router<crate::router::AppState> {
    Router::new()
        .route("/runs", post(create_run))
        .route("/runs/{runId}", get(get_run))
        .route("/runs/{runId}/events", get(stream_run_events))
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
        (status = 202, description = "Accepted async run", body = AsyncRunResponse),
        (status = 400, description = "Invalid request", body = crate::error::ErrorResponse),
        (status = 404, description = "Scope or input file not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
async fn create_run(
    State(run_service): State<Arc<RunService>>,
    Path(scope): Path<Scope>,
    request: Result<Json<CreateRunRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let response = run_service.create_run(scope.clone(), parse_json(request)?).await?;
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
    Ok(http_response)
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
    get,
    path = "/api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/events",
    tag = "runs",
    params(
        Scope,
        ("runId" = String, Path, description = "Run identifier"),
        ("after" = Option<i64>, Query, description = "Resume after the provided event sequence")
    ),
    responses(
        (status = 200, description = "Server-sent run events", content_type = "text/event-stream"),
        (status = 404, description = "Run not found", body = crate::error::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::error::ErrorResponse)
    )
)]
async fn stream_run_events(
    State(run_service): State<Arc<RunService>>,
    Path(path): Path<RunPath>,
    Query(query): Query<RunEventsQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let after_seq = resolve_after_seq(query.after, &headers)?;
    let scope = path.scope();
    let run_id = path.run_id.clone();
    let feed = run_service
        .subscribe_run_events(&scope, &run_id, after_seq)
        .await?;
    let stream_run_service = Arc::clone(&run_service);
    let stream_run_id = run_id.clone();
    let stream_scope = scope.clone();

    let events = stream! {
        let mut delivered_seq = after_seq.unwrap_or(0);

        for event in feed.replay {
            delivered_seq = event.seq();
            yield Ok::<Event, Infallible>(sse_event(&stream_run_id, &event));
            if matches!(event, RunEvent::Complete { .. }) {
                return;
            }
        }

        let Some(mut receiver) = feed.subscription else {
            return;
        };

        loop {
            match receiver.recv().await {
                Ok(event) => {
                    delivered_seq = event.seq();
                    yield Ok::<Event, Infallible>(sse_event(&stream_run_id, &event));
                    if matches!(event, RunEvent::Complete { .. }) {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
                | Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    let catchup = match stream_run_service
                        .subscribe_run_events(&stream_scope, &stream_run_id, Some(delivered_seq))
                        .await
                    {
                        Ok(catchup) => catchup,
                        Err(_) => return,
                    };

                    for event in catchup.replay {
                        delivered_seq = event.seq();
                        yield Ok::<Event, Infallible>(sse_event(&stream_run_id, &event));
                        if matches!(event, RunEvent::Complete { .. }) {
                            return;
                        }
                    }

                    let Some(next_receiver) = catchup.subscription else {
                        return;
                    };
                    receiver = next_receiver;
                }
            }
        }
    };

    let mut response = Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    Ok(response)
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

#[derive(Deserialize)]
struct RunEventsQuery {
    after: Option<i64>,
}

fn map_websocket_rejection(error: WebSocketUpgradeRejection) -> AppError {
    AppError::request(error.to_string())
}

fn parse_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, AppError> {
    request
        .map(|Json(value)| value)
        .map_err(|error| AppError::request(error.body_text()))
}

fn resolve_after_seq(after_query: Option<i64>, headers: &HeaderMap) -> Result<Option<i64>, AppError> {
    if let Some(after_query) = after_query {
        return Ok(Some(after_query));
    }

    let Some(value) = headers.get("last-event-id") else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| AppError::request("Invalid Last-Event-ID header."))?;
    let parsed = value
        .parse::<i64>()
        .map_err(|_| AppError::request("Invalid Last-Event-ID header."))?;
    Ok(Some(parsed))
}

fn sse_event(run_id: &str, event: &RunEvent) -> Event {
    let (event_name, id, data) = RunService::map_public_event(run_id, event)
        .unwrap_or_else(|_| ("run.error", String::new(), "{\"message\":\"Failed to encode a run event.\"}".to_string()));
    let event = Event::default().event(event_name).data(data);
    if id.is_empty() {
        return event;
    }
    event.id(id)
}
