use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(
    crate::routes::system::api_root,
    crate::routes::system::healthz,
    crate::routes::system::readyz,
    crate::routes::system::version,
    crate::routes::uploads::create_upload,
    crate::routes::runs::create_run,
    crate::routes::runs::get_run,
    crate::routes::runs::stream_run_events,
    crate::routes::runs::cancel_run,
    crate::routes::terminal::connect_terminal,
))]
pub struct ApiDoc;
