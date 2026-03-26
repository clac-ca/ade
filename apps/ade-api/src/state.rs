use std::path::PathBuf;

use crate::{config::BuildInfo, readiness::ReadinessController};

#[derive(Clone, Debug)]
pub struct AppState {
    pub build_info: BuildInfo,
    pub readiness: ReadinessController,
    pub web_root: Option<PathBuf>,
}
