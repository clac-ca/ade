use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(
    crate::api::public::system::api_root,
    crate::api::public::system::healthz,
    crate::api::public::system::readyz,
    crate::api::public::system::version,
    crate::api::public::uploads::create_upload,
    crate::api::public::uploads::create_upload_batch,
    crate::api::public::runs::create_run,
    crate::api::public::runs::get_run,
    crate::api::public::runs::create_download,
    crate::api::public::runs::stream_run_events,
    crate::api::public::runs::cancel_run,
    crate::api::public::terminal::connect_terminal,
))]
pub struct ApiDoc;
