use std::path::Path;

use axum::{
    Router,
    body::Body,
    extract::{OriginalUri, Request as AxumRequest, State},
    http::{HeaderMap, Method, Request as HttpRequest, StatusCode, Version},
    response::Response,
    routing::get,
};
use tower::Layer;
use tower::util::ServiceExt;
use tower_http::{
    normalize_path::{NormalizePath, NormalizePathLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use crate::{
    error::AppError,
    routes::{session, system},
    state::AppState,
};

pub fn create_app(state: AppState) -> Router {
    let api_router = Router::new()
        .route("/", get(system::api_root))
        .route("/healthz", get(system::healthz))
        .route("/readyz", get(system::readyz))
        .route("/version", get(system::version))
        .nest(
            "/workspaces/{workspaceId}/configs/{configVersionId}",
            session::router(),
        )
        .fallback(api_not_found);

    Router::new()
        .nest("/api", api_router)
        .fallback(spa_or_not_found)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub fn normalize_app(app: Router) -> NormalizePath<Router> {
    NormalizePathLayer::trim_trailing_slash().layer(app)
}

async fn api_not_found(original_uri: OriginalUri, request: AxumRequest) -> AppError {
    not_found_for_path(&request, original_uri.0.path())
}

async fn spa_or_not_found(
    State(state): State<AppState>,
    request: AxumRequest,
) -> Result<Response, AppError> {
    let request_path = request.uri().path().to_string();
    let request_method = request.method().clone();

    if request_path == "/api" || request_path.starts_with("/api/") {
        return Err(not_found_for_method_path(&request_method, &request_path));
    }

    let Some(web_root) = state.web_root else {
        return Err(not_found_for_method_path(&request_method, &request_path));
    };

    if !matches!(request_method, Method::GET | Method::HEAD) {
        return Err(not_found_for_method_path(&request_method, &request_path));
    }

    let request_version = request.version();
    let request_headers = request.headers().clone();
    let static_response = serve_path(&web_root, request).await?;

    if static_response.status() != StatusCode::NOT_FOUND {
        return Ok(static_response);
    }

    if request_path == "/" || !has_extension(&request_path) {
        return serve_index_html(&web_root, request_method, request_version, request_headers).await;
    }

    Err(not_found_for_method_path(&request_method, &request_path))
}

async fn serve_path(web_root: &Path, request: AxumRequest) -> Result<Response, AppError> {
    let response = ServeDir::new(web_root).oneshot(request).await.unwrap();
    Ok(response.map(Body::new))
}

async fn serve_index_html(
    web_root: &Path,
    method: Method,
    version: Version,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let mut request = HttpRequest::builder()
        .method(method)
        .uri("/index.html")
        .version(version)
        .body(Body::empty())
        .map_err(|error| {
            AppError::internal_with_source(
                "Failed to build the SPA fallback request.".to_string(),
                error,
            )
        })?;
    *request.headers_mut() = headers;

    let response = ServeFile::new(web_root.join("index.html"))
        .oneshot(request)
        .await
        .unwrap();
    Ok(response.map(Body::new))
}

fn has_extension(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|segment| segment.contains('.'))
}

fn not_found_for_method_path(method: &Method, request_path: &str) -> AppError {
    AppError::not_found(format!("Route {method}:{request_path} not found"))
}

fn not_found_for_path(request: &AxumRequest, request_path: &str) -> AppError {
    not_found_for_method_path(request.method(), request_path)
}
