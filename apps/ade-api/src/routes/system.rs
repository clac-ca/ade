use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;

use crate::{config::VersionInfo, readiness::is_application_ready, state::AppState};

#[derive(Debug, Serialize)]
pub struct ServiceStatusResponse {
    pub service: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct RootResponse {
    pub service: &'static str,
    pub status: &'static str,
    pub version: String,
}

pub async fn api_root(State(state): State<AppState>) -> Json<RootResponse> {
    Json(RootResponse {
        service: "ade",
        status: "ok",
        version: state.build_info.version,
    })
}

pub async fn healthz() -> Json<ServiceStatusResponse> {
    Json(ServiceStatusResponse {
        service: "ade",
        status: "ok",
    })
}

pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let snapshot = state.readiness.snapshot();
    let now = crate::unix_time_ms();

    if !is_application_ready(&snapshot, now) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ServiceStatusResponse {
                service: "ade",
                status: "not-ready",
            }),
        );
    }

    (
        StatusCode::OK,
        Json(ServiceStatusResponse {
            service: "ade",
            status: "ready",
        }),
    )
}

pub async fn version(State(state): State<AppState>) -> Json<VersionInfo> {
    Json(VersionInfo {
        build_info: state.build_info,
        runtime_version: format!("rustc {}", rustc_version_runtime::version()),
    })
}
