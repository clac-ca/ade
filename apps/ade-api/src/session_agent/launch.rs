use serde::Serialize;

use crate::error::AppError;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionAgentLaunchConfig {
    pub(crate) bridge_url: String,
    pub(crate) idle_shutdown_seconds: u64,
}

pub(crate) fn encode_launch_config(config: &SessionAgentLaunchConfig) -> Result<Vec<u8>, AppError> {
    serde_json::to_vec(config).map_err(|error| {
        AppError::internal_with_source(
            "Failed to encode the session-agent launch configuration.",
            error,
        )
    })
}

pub(crate) fn render_launch_command(agent_session_path: &str, config_session_path: &str) -> String {
    format!(
        "set -eu\nchmod 755 {agent_session_path}\nexec {agent_session_path} --config-file {config_session_path}"
    )
}

#[cfg(test)]
mod tests {
    use super::{SessionAgentLaunchConfig, encode_launch_config, render_launch_command};

    #[test]
    fn launch_command_uses_config_file_and_execs_the_agent() {
        let command = render_launch_command(
            "/mnt/data/.ade/bin/ade-session-agent",
            "/mnt/data/.ade/session-agent.json",
        );

        assert!(command.contains("ade-session-agent"));
        assert!(command.contains("--config-file"));
    }

    #[test]
    fn launch_config_serializes_to_json() {
        let encoded = encode_launch_config(&SessionAgentLaunchConfig {
            bridge_url: "wss://example.com/api/internal/session-agents/channel".to_string(),
            idle_shutdown_seconds: 10,
        })
        .unwrap();

        let text = String::from_utf8(encoded).unwrap();
        assert!(text.contains("bridgeUrl"));
    }
}
