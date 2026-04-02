//! Agent factory: stateless bridge spawner.
//!
//! Resolves agent kind → provider → AcpBridge. No cache, no registry.
//! The caller (ACPHub/ACPPod) owns the bridge after creation.

use std::sync::Arc;

use anyhow::Context;

pub mod agents;
pub mod provider;
pub mod runtime;

use self::provider::{provider_for_kind, AgentKind};
use self::runtime::{AcpBridge, BridgeClientHandler, BridgeReady};

/// Spawn a new AcpBridge for the given agent kind.
///
/// This is a stateless factory function — it creates a bridge and returns it.
/// The caller owns the bridge and is responsible for its lifecycle.
pub async fn spawn_bridge(
    channel_kind: &str,
    cli_kind: &str,
    workspace: &std::path::Path,
    resume_session_id: Option<String>,
    client_handler: Arc<dyn BridgeClientHandler>,
) -> anyhow::Result<BridgeReady> {
    std::fs::create_dir_all(workspace)
        .with_context(|| format!("Failed to create workspace {:?}", workspace))?;

    let kind = AgentKind::from_str_loose(cli_kind).unwrap_or(AgentKind::Claude);
    let provider = provider_for_kind(kind);

    let ready = AcpBridge::spawn(
        provider,
        kind,
        workspace,
        resume_session_id,
        client_handler,
    )
    .await?;

    eprintln!(
        "[agent_factory] spawned bridge: kind={} channel={}",
        cli_kind, channel_kind
    );

    Ok(ready)
}
