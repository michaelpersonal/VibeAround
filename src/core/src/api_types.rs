//! Typed API request/response shapes shared between the dashboard server
//! and frontend clients. Every struct here has TS bindings emitted to
//! `src/shared/client-ts/generated/`.

use serde::Serialize;

use crate::agent_factory::provider::AgentKind;

/// Per-agent display info returned by `GET /api/agents`.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AgentInfo {
    pub id: AgentKind,
    pub name: String,
    pub description: String,
}

/// `GET /api/agents` response envelope.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AgentsConfig {
    pub agents: Vec<AgentInfo>,
    /// ID of the agent picked by default when the user hasn't selected one.
    /// Typed as `String` (not `AgentKind`) because the server does not
    /// validate this against the enabled set — the frontend should treat an
    /// unrecognized value as "no default".
    pub default_agent: String,
}

impl AgentInfo {
    /// Build an `AgentInfo` for each of the given kinds by looking up the
    /// corresponding entry in `agents.json`.
    pub fn for_kinds(kinds: &[AgentKind]) -> Vec<Self> {
        kinds
            .iter()
            .filter_map(|&kind| {
                let def = crate::resources::agent_by_id(&kind.to_string())?;
                Some(Self {
                    id: kind,
                    name: def.display_name.clone(),
                    description: def.description.clone(),
                })
            })
            .collect()
    }
}
