use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    config::{SERVICE_NAME, SERVICE_VERSION},
    readiness::{ReadinessController, is_application_ready},
};

pub fn router() -> Router<crate::api::AppState> {
    Router::new()
        .route("/", get(api_root))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/version", get(version))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceStatusResponse {
    pub service: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RootResponse {
    pub service: &'static str,
    pub status: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionResponse {
    pub service: &'static str,
    pub version: &'static str,
}

#[utoipa::path(
    get,
    path = "/api",
    tag = "system",
    responses(
        (status = 200, description = "API root", body = RootResponse)
    )
)]
pub async fn api_root() -> Json<RootResponse> {
    Json(RootResponse {
        service: SERVICE_NAME,
        status: "ok",
        version: SERVICE_VERSION,
    })
}

#[utoipa::path(
    get,
    path = "/api/healthz",
    tag = "system",
    responses(
        (status = 200, description = "Service health", body = ServiceStatusResponse)
    )
)]
pub async fn healthz() -> Json<ServiceStatusResponse> {
    Json(ServiceStatusResponse {
        service: SERVICE_NAME,
        status: "ok",
    })
}

#[utoipa::path(
    get,
    path = "/api/readyz",
    tag = "system",
    responses(
        (status = 200, description = "Service is ready", body = ServiceStatusResponse),
        (status = 503, description = "Service is not ready", body = ServiceStatusResponse)
    )
)]
pub async fn readyz(State(readiness): State<ReadinessController>) -> impl IntoResponse {
    let snapshot = readiness.snapshot();
    let now = crate::unix_time_ms();

    if !is_application_ready(&snapshot, now) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ServiceStatusResponse {
                service: SERVICE_NAME,
                status: "not-ready",
            }),
        );
    }

    (
        StatusCode::OK,
        Json(ServiceStatusResponse {
            service: SERVICE_NAME,
            status: "ready",
        }),
    )
}

#[utoipa::path(
    get,
    path = "/api/version",
    tag = "system",
    responses(
        (status = 200, description = "Service version", body = VersionResponse)
    )
)]
pub async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        service: SERVICE_NAME,
        version: SERVICE_VERSION,
    })
}
