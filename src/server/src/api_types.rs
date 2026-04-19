//! HTTP/WebSocket API response shapes for the dashboard.
//!
//! This module owns the **wire contract** between the server and its
//! frontends (web dashboard, Tauri desktop-ui, plus any future TUI / CLI
//! / third-party consumer). Types here exist only to be serialized.
//!
//! # Where the data comes from
//!
//! Structs in this module are populated by reading `common` core state
//! (via `config::ensure_loaded()` and `resources::...`). The core does
//! not know about HTTP; it exposes domain data and this module maps it
//! to wire shapes. Consumers that aren't HTTP (TUI, CLI) should write
//! their own mapping alongside core, not reuse these types.
//!
//! # Consumers
//!
//! The canonical TS validator/types live in
//! `src/shared/client-ts/src/schemas.ts` (zod). Keep the wire shapes
//! documented on each struct below so Python/Swift/curl consumers can
//! derive their own schemas without reading the zod file.

use serde::Serialize;

/// Per-agent display info returned under `AgentsConfig.agents`.
///
/// # Wire format (JSON)
/// ```json
/// { "id": "claude", "name": "Claude Code", "description": "Claude Code CLI" }
/// ```
///
/// - `id`: an agent ID from `resources/agents.json` (e.g. `"claude"`,
///   `"gemini"`, `"qwen-code"`).
/// - `name` / `description`: copied from that file's `display_name` and
///   `description` fields.
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// `GET /api/agents` response envelope.
///
/// # Wire format (JSON)
/// ```json
/// {
///   "agents": [
///     { "id": "claude", "name": "Claude Code", "description": "..." },
///     { "id": "gemini", "name": "Gemini CLI",  "description": "..." }
///   ],
///   "default_agent": "claude"
/// }
/// ```
///
/// - `agents`: the enabled subset from settings.json (not all agents in
///   `agents.json`), ordered as configured.
/// - `default_agent`: raw string from settings.json. The server does not
///   cross-validate against `agents` — consumers should treat an
///   unrecognized value as "no default".
#[derive(Debug, Clone, Serialize)]
pub struct AgentsConfig {
    pub agents: Vec<AgentInfo>,
    pub default_agent: String,
}

impl AgentInfo {
    /// Build an `AgentInfo` for each of the given agent IDs by looking up
    /// the corresponding entry in `agents.json`. IDs with no matching
    /// entry are silently dropped.
    pub fn for_ids(ids: &[String]) -> Vec<Self> {
        ids.iter()
            .filter_map(|id| {
                let def = common::resources::agent_by_id(id)?;
                Some(Self {
                    id: id.clone(),
                    name: def.display_name.clone(),
                    description: def.description.clone(),
                })
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Per-domain runtime shapes (Phase 1g). Each is returned by a dedicated
// `/api/<domain>` handler reading directly from the relevant kernel
// manager — no unified `StatusSnapshot` envelope, no Services facade.
// ---------------------------------------------------------------------------

/// One channel plugin, as returned by `GET /api/channels`.
///
/// Sources: `common::channel_manager::monitor::ChannelMonitor::list()`
///
/// # Wire format (JSON)
/// ```json
/// {
///   "kind": "telegram",
///   "status": "running",
///   "reason": null,
///   "crash_count": 0,
///   "last_seen_age_secs": 3,
///   "restart_in_secs": 0,
///   "started_at": 1713460000
/// }
/// ```
///
/// `status` is one of: `"not_started" | "spawning" | "running" | "crashed" | "stopped"`.
/// `reason` carries a short explanation for crashed/stopped states.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelRuntime {
    pub kind: String,
    pub status: &'static str,
    pub reason: Option<String>,
    pub crash_count: u32,
    pub last_seen_age_secs: u64,
    pub restart_in_secs: u64,
    pub started_at: u64,
}

/// One tunnel, as returned by `GET /api/tunnels`.
///
/// Sources: `common::tunnels::TunnelManager::list()`.
///
/// # Wire format (JSON)
/// ```json
/// {
///   "provider": "localtunnel",
///   "url": "https://quiet-pig-42.loca.lt",
///   "status": { "state": "running" },
///   "uptime_secs": 120
/// }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct TunnelRuntime {
    pub provider: &'static str,
    pub url: Option<String>,
    pub status: common::service::ApiServiceStatus,
    pub uptime_secs: u64,
}

/// One agent runtime, as returned by `GET /api/agents/runtime`.
///
/// Sources: `common::acp_hub::ACPHub::list()` → `ACPPod::state()`.
///
/// # Wire format (JSON)
/// ```json
/// {
///   "route_key": "telegram:chat_42",
///   "channel_kind": "telegram",
///   "chat_id": "chat_42",
///   "cli_kind": "claude",
///   "profile": "default",
///   "session_id": "01HXYZ...",
///   "workspace": "/Users/foo/bar",
///   "busy": false,
///   "failed": null,
///   "started_at": 1713460000,
///   "agent_name": "Claude Code",
///   "agent_title": "Claude",
///   "agent_version": "1.0.0"
/// }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct AgentRuntime {
    pub route_key: String,
    pub channel_kind: String,
    pub chat_id: String,
    pub cli_kind: Option<String>,
    pub profile: Option<String>,
    pub session_id: Option<String>,
    pub workspace: Option<String>,
    pub busy: bool,
    pub failed: Option<String>,
    pub started_at: u64,
    pub agent_name: Option<String>,
    pub agent_title: Option<String>,
    pub agent_version: Option<String>,
}
