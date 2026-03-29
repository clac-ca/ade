use std::path::PathBuf;
use std::sync::Arc;

use crate::readiness::ReadinessController;
use crate::session::SessionService;

#[derive(Clone)]
pub struct AppState {
    pub readiness: ReadinessController,
    pub session_service: Arc<SessionService>,
    pub web_root: Option<PathBuf>,
}
