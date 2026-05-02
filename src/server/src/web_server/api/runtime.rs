use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use common::config;
use common::pty::SessionId;
use common::state::StateSource;

use crate::web_server::AppState;

/// GET /api/agents -- list enabled agents and default agent for frontend agent selector.
pub async fn list_agents_handler() -> Json<crate::api_types::AgentsConfig> {
    let cfg = config::ensure_loaded();
    Json(crate::api_types::AgentsConfig {
        agents: crate::api_types::AgentInfo::for_ids(&cfg.enabled_agents),
        default_agent: cfg.default_agent.clone(),
    })
}

/// GET /api/channels -- live list of channel plugins from `ChannelMonitor`.
pub async fn list_channels_handler(
    State(state): State<AppState>,
) -> Json<Vec<crate::api_types::ChannelRuntime>> {
    let monitor = state.channel_hub.monitor();
    let entries = monitor.list().await;
    Json(
        entries
            .into_iter()
            .map(|s| crate::api_types::ChannelRuntime {
                kind: s.kind,
                status: s.status.as_str(),
                reason: if s.reason.is_empty() {
                    None
                } else {
                    Some(s.reason)
                },
                crash_count: s.crash_count,
                last_seen_age_secs: s.last_seen_age_secs,
                restart_in_secs: s.restart_in_secs,
                started_at: s.started_at,
            })
            .collect(),
    )
}

/// GET /api/tunnels -- live list of tunnels from `TunnelManager`.
pub async fn list_tunnels_handler(
    State(state): State<AppState>,
) -> Json<Vec<crate::api_types::TunnelRuntime>> {
    let entries = state.tunnels.list().await;
    Json(
        entries
            .into_iter()
            .map(|t| crate::api_types::TunnelRuntime {
                provider: t.provider.as_str(),
                url: t.url,
                status: t.status,
                uptime_secs: t.uptime_secs,
            })
            .collect(),
    )
}

/// GET /api/agents/runtime -- live list of agent pods from `ConversationManager`.
pub async fn list_agents_runtime_handler(
    State(state): State<AppState>,
) -> Json<Vec<crate::api_types::AgentRuntime>> {
    let conversation_manager = state.channel_hub.conversation_manager();
    let pods = conversation_manager.list();
    let mut out = Vec::with_capacity(pods.len());
    for pod in pods {
        let st = pod.state().await;
        let (agent_name, agent_title, agent_version) = st
            .initialize
            .as_ref()
            .and_then(|i| i.agent_info.as_ref())
            .map(|info| {
                (
                    Some(info.name.clone()),
                    info.title.clone(),
                    Some(info.version.clone()),
                )
            })
            .unwrap_or((None, None, None));
        out.push(crate::api_types::AgentRuntime {
            route_key: pod.route.as_key(),
            channel_kind: pod.route.channel_kind.clone(),
            chat_id: pod.route.chat_id.clone(),
            cli_kind: st.cli_kind,
            profile: st.profile,
            session_id: st.session_id,
            workspace: st.workspace,
            busy: st.busy,
            failed: st.failed,
            started_at: pod.started_at(),
            agent_name,
            agent_title,
            agent_version,
        });
    }
    Json(out)
}

/// POST /api/channels/:kind/stop -- user-initiated stop of a channel
/// plugin (no auto-respawn).
pub async fn stop_channel_handler(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> impl IntoResponse {
    match state.channel_hub.monitor().force_stop(&kind).await {
        Ok(()) => (StatusCode::OK, format!("Stopped {}", kind)),
        Err(e) => (StatusCode::NOT_FOUND, e),
    }
}

/// POST /api/channels/:kind/restart -- user-initiated restart (kill +
/// immediate respawn, no 15s backoff).
pub async fn restart_channel_handler(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> impl IntoResponse {
    match state.channel_hub.monitor().force_restart(&kind).await {
        Ok(()) => (StatusCode::OK, format!("Restarting {}", kind)),
        Err(e) => (StatusCode::NOT_FOUND, e),
    }
}

/// POST /api/channels/:kind/start -- transition a Stopped channel
/// back to Crashed(restart_at=now) so the next monitor tick respawns it.
pub async fn start_channel_handler(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> impl IntoResponse {
    match state.channel_hub.monitor().force_start(&kind) {
        Ok(()) => (StatusCode::OK, format!("Starting {}", kind)),
        Err(e) => (StatusCode::NOT_FOUND, e),
    }
}

/// DELETE /api/tunnels/:provider -- kill a running tunnel.
pub async fn kill_tunnel_handler(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> impl IntoResponse {
    if state.tunnels.kill(&provider) {
        (StatusCode::OK, format!("Killed tunnel {}", provider))
    } else {
        (
            StatusCode::NOT_FOUND,
            format!("Tunnel {} not found", provider),
        )
    }
}

/// DELETE /api/agents/:route_key -- close an agent pod.
///
/// `route_key` is the colon-joined form from `RouteKey::as_key()`, e.g.
/// `telegram:chat_42`. The handler closes the pod (shutting down its
/// bridge) and returns `404` if the key doesn't parse.
pub async fn kill_agent_handler(
    State(state): State<AppState>,
    Path(route_key): Path<String>,
) -> impl IntoResponse {
    let Some(route) = common::routing::RouteKey::from_key(&route_key) else {
        return (
            StatusCode::NOT_FOUND,
            format!("Invalid agent route key: {}", route_key),
        );
    };
    state
        .channel_hub
        .conversation_manager()
        .close(&route, Some("killed by user".to_string()))
        .await;
    (StatusCode::OK, format!("Killed agent {}", route_key))
}

/// DELETE /api/pty/:session_id -- kill a PTY session.
///
/// Goes through `PtySessionManager::delete_session` so the child
/// process gets SIGKILL'd, not just the registry entry removed.
pub async fn kill_pty_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let Ok(uuid) = uuid::Uuid::parse_str(&session_id) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("Invalid session id: {}", session_id),
        );
    };
    if state.pty_manager.delete_session(SessionId(uuid)) {
        (StatusCode::OK, format!("Killed pty {}", session_id))
    } else {
        (
            StatusCode::NOT_FOUND,
            format!("PTY session {} not found", session_id),
        )
    }
}
