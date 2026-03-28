use std::path::PathBuf;
use std::sync::Arc;

use crate::readiness::ReadinessController;
use crate::runtime::RuntimeService;

#[derive(Clone)]
pub struct AppState {
    pub readiness: ReadinessController,
    pub runtime: Option<Arc<RuntimeService>>,
    pub web_root: Option<PathBuf>,
}
