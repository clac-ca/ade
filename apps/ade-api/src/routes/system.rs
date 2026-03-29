use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;

use crate::{
    config::{SERVICE_NAME, SERVICE_VERSION},
    readiness::{ReadinessController, is_application_ready},
};

#[derive(Debug, Serialize)]
pub struct ServiceStatusResponse {
    pub service: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct RootResponse {
    pub service: &'static str,
    pub status: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub service: &'static str,
    pub version: &'static str,
}

pub async fn api_root() -> Json<RootResponse> {
    Json(RootResponse {
        service: SERVICE_NAME,
        status: "ok",
        version: SERVICE_VERSION,
    })
}

pub async fn healthz() -> Json<ServiceStatusResponse> {
    Json(ServiceStatusResponse {
        service: SERVICE_NAME,
        status: "ok",
    })
}

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

pub async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        service: SERVICE_NAME,
        version: SERVICE_VERSION,
    })
}
