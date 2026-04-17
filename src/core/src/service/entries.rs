//! Per-kind status entries held in the service registry.

use serde::Serialize;

use crate::tunnels::TunnelProvider;

use super::status::ServiceMeta;

/// Agent status entry (lightweight, for Dashboard display only).
#[derive(Debug, Clone, Serialize)]
pub struct AgentStatusEntry {
    pub key: String,
    pub kind: String,
    pub started_at: u64,
}

/// Channel plugin status entry.
pub struct ChannelEntry {
    pub meta: ServiceMeta,
}

/// Tunnel status entry.
pub struct TunnelEntry {
    pub meta: ServiceMeta,
    pub provider: TunnelProvider,
    pub url: Option<String>,
}
