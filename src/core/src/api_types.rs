//! Typed API request/response shapes shared between the dashboard server
//! and frontend clients. Every struct here has TS bindings emitted to
//! `src/shared/client-ts/generated/`.

use serde::Serialize;

/// Per-agent display info returned by `GET /api/agents`.
///
/// `id` is the agent's identifier from `resources/agents.json`. It is
/// emitted as `string` in the generated TS type; frontends that need the
/// narrow literal union can import `AgentId` from
/// `@va/generated/AgentId` and narrow on demand.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// `GET /api/agents` response envelope.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AgentsConfig {
    pub agents: Vec<AgentInfo>,
    /// ID of the agent picked by default when the user hasn't selected one.
    /// Not cross-validated against `enabled_agents` — frontends should
    /// treat an unknown value as "no default".
    pub default_agent: String,
}

impl AgentInfo {
    /// Build an `AgentInfo` for each of the given agent IDs by looking up
    /// the corresponding entry in `agents.json`. IDs with no matching
    /// entry are silently dropped.
    pub fn for_ids(ids: &[String]) -> Vec<Self> {
        ids.iter()
            .filter_map(|id| {
                let def = crate::resources::agent_by_id(id)?;
                Some(Self {
                    id: id.clone(),
                    name: def.display_name.clone(),
                    description: def.description.clone(),
                })
            })
            .collect()
    }
}
