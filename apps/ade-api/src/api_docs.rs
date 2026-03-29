use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(
    crate::routes::system::api_root,
    crate::routes::system::healthz,
    crate::routes::system::readyz,
    crate::routes::system::version,
    crate::routes::session::execute_command,
    crate::routes::session::upload_file,
    crate::routes::session::list_files,
    crate::routes::session::download_file,
    crate::routes::session::create_run,
))]
pub struct ApiDoc;

#[derive(utoipa::ToSchema)]
pub struct UploadFileBody {
    #[schema(value_type = String, format = Binary)]
    pub file: String,
}
