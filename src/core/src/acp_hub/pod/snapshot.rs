//! `PodSnapshot` — serializable view of an `ACPPod`'s current state.

use serde::Serialize;

use agent_client_protocol as acp;

use crate::acp::routing::RouteKey;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodSnapshot {
    pub route: RouteKey,
    pub bot_identity: Option<String>,
    pub session_id: Option<String>,
    pub cli_kind: Option<String>,
    pub profile: Option<String>,
    pub workspace: Option<String>,
    pub busy: bool,
    pub failed: Option<String>,
    pub started_at: u64,
    pub initialize: Option<acp::InitializeResponse>,
}

impl PodSnapshot {
    pub fn service_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.route.channel_kind,
            self.route.chat_id,
            self.profile
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            self.cli_kind
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        )
    }
}
