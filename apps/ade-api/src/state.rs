use std::path::PathBuf;

use crate::readiness::ReadinessController;

#[derive(Clone, Debug)]
pub struct AppState {
    pub readiness: ReadinessController,
    pub web_root: Option<PathBuf>,
}
