use std::{fs, path::PathBuf};

use ade_api::{
    config::SERVICE_VERSION,
    readiness::{CreateReadinessControllerOptions, ReadinessController, ReadinessPhase},
    router::{create_app, normalize_app},
    state::AppState,
    unix_time_ms,
};
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
    response::Response,
};
use pretty_assertions::assert_eq;
use tower::util::ServiceExt;

fn fixture_web_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/web-dist")
}

async fn json_body(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn request(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn request_with_method(uri: &str, method: Method) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn health_route_reports_ok() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions {
            database_ok: Some(true),
            last_checked_at: Some(unix_time_ms()),
            phase: Some(ReadinessPhase::Ready),
            ..CreateReadinessControllerOptions::default()
        }),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app.oneshot(request("/api/healthz")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await,
        serde_json::json!({
            "service": "ade",
            "status": "ok"
        })
    );
}

#[tokio::test]
async fn api_root_works_without_trailing_slash() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app.oneshot(request("/api")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await,
        serde_json::json!({
            "service": "ade",
            "status": "ok",
            "version": SERVICE_VERSION
        })
    );
}

#[tokio::test]
async fn api_root_works_with_trailing_slash() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app.oneshot(request("/api/")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await,
        serde_json::json!({
            "service": "ade",
            "status": "ok",
            "version": SERVICE_VERSION
        })
    );
}

#[tokio::test]
async fn ready_route_reflects_readiness_state() {
    let readiness = ReadinessController::new(CreateReadinessControllerOptions {
        phase: Some(ReadinessPhase::Starting),
        ..CreateReadinessControllerOptions::default()
    });
    let app = normalize_app(create_app(AppState {
        readiness: readiness.clone(),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let not_ready = app.clone().oneshot(request("/api/readyz")).await.unwrap();
    assert_eq!(not_ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        json_body(not_ready).await,
        serde_json::json!({
            "service": "ade",
            "status": "not-ready"
        })
    );

    readiness.record_database_success(unix_time_ms());
    readiness.mark_ready();

    let ready = app.oneshot(request("/api/readyz")).await.unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(
        json_body(ready).await,
        serde_json::json!({
            "service": "ade",
            "status": "ready"
        })
    );
}

#[tokio::test]
async fn version_route_exposes_minimal_runtime_metadata() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app.oneshot(request("/api/version")).await.unwrap();
    let payload = json_body(response).await;

    assert_eq!(payload["service"], "ade");
    assert_eq!(payload["version"], SERVICE_VERSION);
    assert_eq!(payload.as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn spa_fallback_serves_index_html_for_unknown_frontend_routes() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app.oneshot(request("/documents/example")).await.unwrap();
    let html = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(html.contains("id=\"root\""));
}

#[tokio::test]
async fn head_requests_to_frontend_routes_preserve_head_semantics() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app
        .oneshot(request_with_method("/documents/example", Method::HEAD))
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    assert!(body.is_empty());
}

#[tokio::test]
async fn unknown_api_routes_return_json_404() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app.oneshot(request("/api/unknown")).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(response).await,
        serde_json::json!({
            "error": "Not Found",
            "message": "Route GET:/api/unknown not found",
            "statusCode": 404
        })
    );
}

#[tokio::test]
async fn missing_assets_stay_not_found() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app.oneshot(request("/assets/missing.js")).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_requests_to_missing_assets_stay_not_found() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app
        .oneshot(request_with_method("/assets/missing.js", Method::HEAD))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn root_serves_application_shell_when_web_root_exists() {
    let app = normalize_app(create_app(AppState {
        readiness: ReadinessController::new(CreateReadinessControllerOptions::default()),
        runtime: None,
        web_root: Some(fixture_web_root()),
    }));

    let response = app.oneshot(request("/")).await.unwrap();
    let html = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(html.contains("id=\"root\""));
}

#[test]
fn fixture_files_exist() {
    let web_root = fixture_web_root();
    assert!(fs::metadata(web_root.join("index.html")).is_ok());
    assert!(fs::metadata(web_root.join("assets/app.js")).is_ok());
}
