use std::{path::PathBuf, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{FromRef, OriginalUri, Request as AxumRequest, State},
    http::{Method, Request as HttpRequest, StatusCode},
    response::Response,
    routing::get,
};
use tower::util::ServiceExt;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    api::{docs::ApiDoc, internal, public},
    error::AppError,
    readiness::ReadinessController,
    runs::RunService,
    sandbox_environment::SandboxEnvironmentManager,
    terminal::TerminalService,
};

#[derive(Clone)]
pub struct AppState {
    pub readiness: ReadinessController,
    pub sandbox_environment_manager: Arc<SandboxEnvironmentManager>,
    pub run_service: Arc<RunService>,
    pub terminal_service: Arc<TerminalService>,
    pub web_root: Option<PathBuf>,
}

impl FromRef<AppState> for ReadinessController {
    fn from_ref(state: &AppState) -> Self {
        state.readiness.clone()
    }
}

impl FromRef<AppState> for Arc<RunService> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.run_service)
    }
}

impl FromRef<AppState> for Arc<SandboxEnvironmentManager> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.sandbox_environment_manager)
    }
}

impl FromRef<AppState> for Arc<TerminalService> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.terminal_service)
    }
}

pub fn create_app(state: AppState) -> Router {
    let openapi = ApiDoc::openapi();
    let api_router = public::system::router()
        .nest("/internal", internal::router())
        .nest(
            "/workspaces/{workspaceId}/configs/{configVersionId}",
            public::scoped_router(),
        )
        .fallback(
            |original_uri: OriginalUri, request: AxumRequest| async move {
                not_found_for_method_path(request.method(), original_uri.0.path())
            },
        );

    Router::new()
        .route("/api/", get(public::system::api_root))
        .nest("/api", api_router)
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", openapi))
        .fallback(spa_or_not_found)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
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
    let static_response = ServeDir::new(&web_root)
        .oneshot(request)
        .await
        .unwrap()
        .map(Body::new);

    if static_response.status() != StatusCode::NOT_FOUND {
        return Ok(static_response);
    }

    if request_path == "/"
        || request_path
            .rsplit('/')
            .next()
            .is_some_and(|segment| !segment.contains('.'))
    {
        let mut request = HttpRequest::builder()
            .method(request_method)
            .uri("/index.html")
            .version(request_version)
            .body(Body::empty())
            .map_err(|error| {
                AppError::internal_with_source(
                    "Failed to build the SPA fallback request.".to_string(),
                    error,
                )
            })?;
        *request.headers_mut() = request_headers;

        let response = ServeFile::new(web_root.join("index.html"))
            .oneshot(request)
            .await
            .unwrap();
        return Ok(response.map(Body::new));
    }

    Err(not_found_for_method_path(&request_method, &request_path))
}

fn not_found_for_method_path(method: &Method, request_path: &str) -> AppError {
    AppError::not_found(format!("Route {method}:{request_path} not found"))
}
