//! Agent factory: stateless bridge spawner.
//!
//! Resolves agent kind → provider → AcpBridge. No cache, no registry.
//! The caller (ACPHub/ACPPod) owns the bridge after creation.

use std::sync::Arc;

use anyhow::Context;

pub mod provider;
pub mod runtime;

use self::provider::provider_for_id;
use self::runtime::{AcpBridge, BridgeClientHandler, BridgeReady};

/// Spawn a new AcpBridge for the given agent.
///
/// Resolves `cli_kind` against `agents.json` (accepting primary IDs and
/// aliases); falls back to `"claude"` if unrecognized. This is a stateless
/// factory function — the caller owns the bridge and is responsible for
/// its lifecycle.
pub async fn spawn_bridge(
    channel_kind: &str,
    chat_id: &str,
    cli_kind: &str,
    workspace: &std::path::Path,
    resume_session_id: Option<String>,
    client_handler: Arc<dyn BridgeClientHandler>,
) -> anyhow::Result<BridgeReady> {
    std::fs::create_dir_all(workspace)
        .with_context(|| format!("Failed to create workspace {:?}", workspace))?;

    let agent_id = crate::resources::agent_by_alias(cli_kind)
        .map(|def| def.id.clone())
        .unwrap_or_else(|| "claude".to_string());
    let provider = provider_for_id(agent_id.clone());

    // VibeAround-specific env vars so skills can resolve session context.
    let env_vars = vec![
        ("VIBEAROUND_CHANNEL_KIND".to_string(), channel_kind.to_string()),
        ("VIBEAROUND_CHAT_ID".to_string(), chat_id.to_string()),
        ("VIBEAROUND_AGENT_KIND".to_string(), agent_id.clone()),
    ];

    let ready = AcpBridge::spawn(
        provider,
        agent_id.clone(),
        workspace,
        resume_session_id,
        client_handler,
        env_vars,
    )
    .await?;

    eprintln!(
        "[agent_factory] spawned bridge: agent_id={} channel={}",
        agent_id, channel_kind
    );

    Ok(ready)
}
