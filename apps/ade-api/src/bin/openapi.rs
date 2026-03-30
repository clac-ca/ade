use ade_api::api::docs::ApiDoc;
use utoipa::OpenApi;

fn main() {
    let document = ApiDoc::openapi();
    let json = serde_json::to_string_pretty(&document)
        .expect("failed to serialize the generated OpenAPI document");
    println!("{json}");
}
