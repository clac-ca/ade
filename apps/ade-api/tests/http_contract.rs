use std::{fs, path::PathBuf, sync::Arc};

use ade_api::{
    api::{AppState, create_app},
    config::SERVICE_VERSION,
    readiness::{DatabaseReadiness, ReadinessController, ReadinessPhase, ReadinessSnapshot},
    runs::{InMemoryRunStore, RunService},
    scope_session::ScopeSessionService,
    terminal::TerminalService,
    unix_time_ms,
};
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
    response::Response,
};
use pretty_assertions::assert_eq;
use tempfile::tempdir;
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

fn fixture_scope_session_service() -> Arc<ScopeSessionService> {
    let tempdir = tempdir().unwrap();
    let bundle_root = tempdir.path().join("session-bundle");
    fs::create_dir_all(bundle_root.join("bin")).unwrap();
    fs::create_dir_all(bundle_root.join("python")).unwrap();
    fs::create_dir_all(bundle_root.join("wheelhouse/base")).unwrap();
    let connector = bundle_root.join("bin/reverse-connect");
    let prepare = bundle_root.join("bin/prepare.sh");
    let engine = bundle_root.join("wheelhouse/base/ade_engine-0.1.0-py3-none-any.whl");
    let config = tempdir.path().join("ade_config-0.1.0-py3-none-any.whl");
    let toolchain = bundle_root.join("python/python-3.14.0-linux-x86_64.tar.gz");
    std::fs::write(&connector, b"connector").unwrap();
    std::fs::write(&prepare, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(&engine, b"engine").unwrap();
    std::fs::write(&config, b"config").unwrap();
    std::fs::write(&toolchain, b"toolchain").unwrap();

    let env = [
        (
            "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT".to_string(),
            "http://127.0.0.1:9".to_string(),
        ),
        (
            "ADE_SESSION_BUNDLE_ROOT".to_string(),
            bundle_root.display().to_string(),
        ),
        (
            "ADE_SESSION_SECRET".to_string(),
            "test-session-secret".to_string(),
        ),
        (
            "ADE_CONFIG_TARGETS".to_string(),
            serde_json::json!([
                {
                    "workspaceId": "workspace-a",
                    "configVersionId": "config-v1",
                    "wheelPath": config.display().to_string(),
                }
            ])
            .to_string(),
        ),
        (
            "ADE_APP_URL".to_string(),
            "http://127.0.0.1:8000".to_string(),
        ),
    ]
    .into_iter()
    .collect();
    std::mem::forget(tempdir);

    Arc::new(ScopeSessionService::from_env(&env).unwrap())
}

fn fixture_terminal_service(
    scope_session_service: Arc<ScopeSessionService>,
) -> Arc<TerminalService> {
    let env = [(
        "ADE_APP_URL".to_string(),
        "http://127.0.0.1:8000".to_string(),
    )]
    .into_iter()
    .collect();

    Arc::new(TerminalService::from_env(&env, scope_session_service).unwrap())
}

fn app_state(readiness: ReadinessController) -> AppState {
    let scope_session_service = fixture_scope_session_service();
    let env = [
        (
            "ADE_APP_URL".to_string(),
            "http://127.0.0.1:8000".to_string(),
        ),
        (
            "ADE_BLOB_ACCOUNT_URL".to_string(),
            "http://127.0.0.1:65535/devstoreaccount1".to_string(),
        ),
        (
            "ADE_BLOB_CONTAINER".to_string(),
            "documents".to_string(),
        ),
        (
            "ADE_BLOB_ACCOUNT_KEY".to_string(),
            "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=="
                .to_string(),
        ),
    ]
    .into_iter()
    .collect();
    AppState {
        readiness,
        run_service: Arc::new(
            RunService::from_env(
                &env,
                Arc::clone(&scope_session_service),
                Arc::new(InMemoryRunStore::default()),
            )
            .unwrap(),
        ),
        scope_session_service: Arc::clone(&scope_session_service),
        terminal_service: fixture_terminal_service(Arc::clone(&scope_session_service)),
        web_root: Some(fixture_web_root()),
    }
}

#[tokio::test]
async fn health_route_reports_ok() {
    let app = create_app(app_state(ReadinessController::new(ReadinessSnapshot {
        database: DatabaseReadiness {
            ok: true,
            last_checked_at: Some(unix_time_ms()),
            ..DatabaseReadiness::default()
        },
        phase: ReadinessPhase::Ready,
    })));

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
    let app = create_app(app_state(ReadinessController::default()));

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
    let app = create_app(app_state(ReadinessController::default()));

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
    let readiness = ReadinessController::new(ReadinessSnapshot {
        phase: ReadinessPhase::Starting,
        ..ReadinessSnapshot::default()
    });
    let app = create_app(app_state(readiness.clone()));

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
    let app = create_app(app_state(ReadinessController::default()));

    let response = app.oneshot(request("/api/version")).await.unwrap();
    let payload = json_body(response).await;

    assert_eq!(payload["service"], "ade");
    assert_eq!(payload["version"], SERVICE_VERSION);
    assert_eq!(payload.as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn openapi_route_serves_generated_spec() {
    let app = create_app(app_state(ReadinessController::default()));

    let response = app.oneshot(request("/api/openapi.json")).await.unwrap();
    let payload = json_body(response).await;

    assert_eq!(payload["openapi"], "3.1.0");
    assert!(payload["paths"]["/api/healthz"].is_object());
    assert!(
        payload["paths"]["/api/workspaces/{workspaceId}/configs/{configVersionId}/uploads"]
            .is_object()
    );
    assert!(
        payload["paths"]["/api/workspaces/{workspaceId}/configs/{configVersionId}/uploads/batches"]
            .is_object()
    );
    assert!(
        payload["paths"]["/api/workspaces/{workspaceId}/configs/{configVersionId}/runs/{runId}/events"]
            .is_object()
    );
    assert!(
        payload["paths"]["/api/workspaces/{workspaceId}/configs/{configVersionId}/terminal"]
            .is_object()
    );
    assert!(
        payload["paths"]["/api/workspaces/{workspaceId}/configs/{configVersionId}/files"].is_null()
    );
    assert!(
        payload["paths"]["/api/workspaces/{workspaceId}/configs/{configVersionId}/executions"]
            .is_null()
    );
}

#[tokio::test]
async fn docs_route_serves_swagger_ui() {
    let app = create_app(app_state(ReadinessController::default()));

    let redirect = app.clone().oneshot(request("/api/docs")).await.unwrap();
    assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
    assert_eq!(redirect.headers().get("location").unwrap(), "/api/docs/");

    let response = app.clone().oneshot(request("/api/docs/")).await.unwrap();
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Swagger UI"));

    let response = app.oneshot(request("/api/docs/index.html")).await.unwrap();
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Swagger UI"));
}

#[tokio::test]
async fn spa_fallback_serves_index_html_for_unknown_frontend_routes() {
    let app = create_app(app_state(ReadinessController::default()));

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
    let app = create_app(app_state(ReadinessController::default()));

    let response = app
        .oneshot(request_with_method("/documents/example", Method::HEAD))
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    assert!(body.is_empty());
}

#[tokio::test]
async fn unknown_api_routes_return_json_404() {
    let app = create_app(app_state(ReadinessController::default()));

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
    let app = create_app(app_state(ReadinessController::default()));

    let response = app.oneshot(request("/assets/missing.js")).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_requests_to_missing_assets_stay_not_found() {
    let app = create_app(app_state(ReadinessController::default()));

    let response = app
        .oneshot(request_with_method("/assets/missing.js", Method::HEAD))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn root_serves_application_shell_when_web_root_exists() {
    let app = create_app(app_state(ReadinessController::default()));

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
