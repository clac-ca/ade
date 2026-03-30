use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, ToSchema, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct Scope {
    /// Workspace id.
    #[serde(rename = "workspaceId")]
    pub workspace_id: String,
    /// Config version id.
    #[serde(rename = "configVersionId")]
    pub config_version_id: String,
}
