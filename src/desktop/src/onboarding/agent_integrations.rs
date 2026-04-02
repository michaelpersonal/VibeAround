//! Agent integration installation — thin wrapper over common::agent_integrations.

/// Sync MCP config + skills for enabled/disabled agents.
pub(super) fn install_agent_integrations(settings: &serde_json::Value) -> anyhow::Result<()> {
    common::agent_integrations::sync_integrations(settings);
    Ok(())
}

/// Pre-install npm-based ACP agent packages for enabled agents.
pub(super) async fn install_acp_agents(settings: &serde_json::Value) {
    common::agent_integrations::install_acp_agents(settings).await;
}
