use std::{convert::Infallible, sync::Arc, time::Duration};

use async_stream::stream;
use axum::{
    Json, Router,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response, sse::Event, sse::KeepAlive, sse::Sse},
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    error::AppError,
    runs::{
        AsyncRunResponse, CreateRunRequest, RunDetailResponse, RunEvent, RunService,
        events::map_public_event,
    },
    scope::Scope,
};

pub fn router() -> Router<crate::api::AppState> {
    Router::new()
        .route("/runs", post(create_run))
        .route("/runs/{runId}", get(get_run))
        .route("/runs/{runId}/events", get(stream_run_events))
        .route("/runs/{runId}/cancel", post(cancel_run))
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
    let request = request
        .map(|Json(value)| value)
        .map_err(|error| AppError::request(error.body_text()))?;
    let response = run_service.create_run(scope.clone(), request).await?;
    let location = format!(
        "/api/workspaces/{}/configs/{}/runs/{}",
        scope.workspace_id, scope.config_version_id, response.run_id
    );
    let mut http_response = (StatusCode::ACCEPTED, Json(response)).into_response();
    http_response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location).map_err(|error| {
            AppError::internal_with_source("Failed to encode the async run location header.", error)
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
    Path((workspace_id, config_version_id, run_id)): Path<(String, String, String)>,
) -> Result<Json<RunDetailResponse>, AppError> {
    let scope = Scope {
        workspace_id,
        config_version_id,
    };
    Ok(Json(run_service.get_run_detail(&scope, &run_id).await?))
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
    Path((workspace_id, config_version_id, run_id)): Path<(String, String, String)>,
    Query(query): Query<RunEventsQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let after_seq = if let Some(after_query) = query.after {
        Some(after_query)
    } else {
        match headers.get("last-event-id") {
            Some(value) => Some(
                value
                    .to_str()
                    .map_err(|_| AppError::request("Invalid Last-Event-ID header."))?
                    .parse::<i64>()
                    .map_err(|_| AppError::request("Invalid Last-Event-ID header."))?,
            ),
            None => None,
        }
    };
    let scope = Scope {
        workspace_id,
        config_version_id,
    };
    let feed = run_service
        .subscribe_run_events(&scope, &run_id, after_seq)
        .await?;
    let stream_run_service = Arc::clone(&run_service);
    let stream_run_id = run_id.clone();
    let stream_scope = scope.clone();
    let sse_event = |run_id: &str, event: &RunEvent| {
        let (event_name, id, data) = map_public_event(run_id, event).unwrap_or_else(|_| {
            (
                "run.error",
                String::new(),
                "{\"message\":\"Failed to encode a run event.\"}".to_string(),
            )
        });
        let event = Event::default().event(event_name).data(data);
        if id.is_empty() {
            return event;
        }
        event.id(id)
    };

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
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
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
    Path((workspace_id, config_version_id, run_id)): Path<(String, String, String)>,
) -> Result<StatusCode, AppError> {
    let scope = Scope {
        workspace_id,
        config_version_id,
    };
    run_service.cancel_run(&scope, &run_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct RunEventsQuery {
    after: Option<i64>,
}
