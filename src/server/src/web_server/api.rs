//! REST API handlers for the web server.
//!
//! - GET /api/sessions
//! - POST /api/sessions
//! - DELETE /api/sessions/:session_id
//! - GET /api/tmux/sessions
//! - GET /api/agents
//! - GET  /api/channels
//! - POST /api/channels/:kind/{stop,restart,start}
//! - GET  /api/tunnels
//! - DELETE /api/tunnels/:provider
//! - GET  /api/agents/runtime
//! - DELETE /api/agents/:route_key
//! - DELETE /api/pty/:session_id

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use common::config;
use common::pty::{list_tmux_sessions, tmux_available, PtyTool, SessionId};
use common::state::StateSource;

use super::AppState;

/// GET /api/tmux/sessions — list active tmux sessions and whether tmux is available.
pub async fn list_tmux_sessions_handler() -> Json<crate::api_types::TmuxSessionsResponse> {
    let available = tmux_available();
    let sessions = if available { list_tmux_sessions() } else { vec![] };
    Json(crate::api_types::TmuxSessionsResponse { available, sessions })
}

/// GET /api/agents — list enabled agents and default agent for frontend agent selector.
pub async fn list_agents_handler() -> Json<crate::api_types::AgentsConfig> {
    let cfg = config::ensure_loaded();
    Json(crate::api_types::AgentsConfig {
        agents: crate::api_types::AgentInfo::for_ids(&cfg.enabled_agents),
        default_agent: cfg.default_agent.clone(),
    })
}

/// GET /api/channels — live list of channel plugins from `ChannelMonitor`.
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
                reason: if s.reason.is_empty() { None } else { Some(s.reason) },
                crash_count: s.crash_count,
                last_seen_age_secs: s.last_seen_age_secs,
                restart_in_secs: s.restart_in_secs,
                started_at: s.started_at,
            })
            .collect(),
    )
}

/// GET /api/tunnels — live list of tunnels from `TunnelManager`.
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

/// GET /api/agents/runtime — live list of agent pods from `ConversationManager`.
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
            .map(|info| (Some(info.name.clone()), info.title.clone(), Some(info.version.clone())))
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

/// POST /api/channels/:kind/stop — user-initiated stop of a channel
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

/// POST /api/channels/:kind/restart — user-initiated restart (kill +
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

/// POST /api/channels/:kind/start — transition a Stopped channel
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

/// DELETE /api/tunnels/:provider — kill a running tunnel.
pub async fn kill_tunnel_handler(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> impl IntoResponse {
    if state.tunnels.kill(&provider) {
        (StatusCode::OK, format!("Killed tunnel {}", provider))
    } else {
        (StatusCode::NOT_FOUND, format!("Tunnel {} not found", provider))
    }
}

/// DELETE /api/agents/:route_key — close an agent pod.
///
/// `route_key` is the colon-joined form from `RouteKey::as_key()`, e.g.
/// `telegram:chat_42`. The handler closes the pod (shutting down its
/// bridge) and returns `404` if the key doesn't parse.
pub async fn kill_agent_handler(
    State(state): State<AppState>,
    Path(route_key): Path<String>,
) -> impl IntoResponse {
    let Some(route) = common::routing::RouteKey::from_key(&route_key) else {
        return (StatusCode::NOT_FOUND, format!("Invalid agent route key: {}", route_key));
    };
    state
        .channel_hub
        .conversation_manager()
        .close(&route, Some("killed by user".to_string()))
        .await;
    (StatusCode::OK, format!("Killed agent {}", route_key))
}

/// DELETE /api/pty/:session_id — kill a PTY session.
///
/// Goes through `PtySessionManager::delete_session` so the child
/// process gets SIGKILL'd, not just the registry entry removed.
pub async fn kill_pty_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let Ok(uuid) = uuid::Uuid::parse_str(&session_id) else {
        return (StatusCode::BAD_REQUEST, format!("Invalid session id: {}", session_id));
    };
    if state.pty_manager.delete_session(SessionId(uuid)) {
        (StatusCode::OK, format!("Killed pty {}", session_id))
    } else {
        (StatusCode::NOT_FOUND, format!("PTY session {} not found", session_id))
    }
}

/// Request body for POST /api/sessions.
#[derive(serde::Deserialize)]
pub(crate) struct CreateSessionBody {
    tool: PtyTool,
    project_path: Option<String>,
    tmux_session: Option<String>,
    theme: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

/// GET /api/sessions — list all active sessions.
pub async fn list_sessions_handler(
    State(state): State<AppState>,
) -> Json<Vec<crate::api_types::SessionListItem>> {
    let items = state
        .pty_manager
        .list_sessions()
        .into_iter()
        .map(|item| crate::api_types::SessionListItem {
            session_id: item.session_id,
            tool: item.tool,
            status: item.status,
            created_at: item.created_at,
            project_path: item.project_path,
            tmux_session: item.tmux_session,
        })
        .collect();
    Json(items)
}

/// POST /api/sessions — create a new PTY session.
pub async fn create_session_handler(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionBody>,
) -> Result<Json<crate::api_types::CreateSessionResponse>, (StatusCode, String)> {
    let initial_size = match (body.cols, body.rows) {
        (Some(c), Some(r)) => Some((c, r)),
        _ => None,
    };

    let created = state
        .pty_manager
        .create_session(
            body.tool,
            body.project_path.clone(),
            body.tmux_session.clone(),
            body.theme.clone(),
            initial_size,
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(crate::api_types::CreateSessionResponse {
        session_id: created.session_id,
        tool: created.tool,
        created_at: created.created_at,
        project_path: created.project_path,
    }))
}

// ---------------------------------------------------------------------------
// Workspace management
// ---------------------------------------------------------------------------

/// GET /api/workspaces — list all workspaces.
pub async fn list_workspaces_handler() -> Json<crate::api_types::WorkspacesResponse> {
    let cfg = config::ensure_loaded();
    let builtin = config::builtin_workspaces_dir();
    let all = cfg.all_workspaces();

    let items = all
        .iter()
        .map(|ws| {
            let is_builtin = *ws == builtin;
            let is_default = cfg.default_workspace.as_ref() == Some(ws)
                || (cfg.default_workspace.is_none() && is_builtin);
            crate::api_types::WorkspaceItem {
                path: ws.to_string_lossy().to_string(),
                is_default,
                is_builtin,
            }
        })
        .collect();

    Json(crate::api_types::WorkspacesResponse {
        workspaces: items,
        default_workspace: cfg
            .default_workspace
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| builtin.to_string_lossy().to_string()),
    })
}

#[derive(serde::Deserialize)]
pub(crate) struct WorkspacePathBody {
    path: String,
}

/// POST /api/workspaces — add a workspace path.
pub async fn add_workspace_handler(
    Json(body): Json<WorkspacePathBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = std::path::PathBuf::from(&body.path);
    if !path.exists() || !path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, format!("Path does not exist or is not a directory: {}", body.path)));
    }
    config::update_settings_json(|root| {
        if let Some(obj) = root.as_object_mut() {
            let workspaces = obj
                .entry("workspaces")
                .or_insert_with(|| serde_json::json!([]));
            if let Some(arr) = workspaces.as_array_mut() {
                let val = serde_json::Value::String(body.path.clone());
                if !arr.contains(&val) {
                    arr.push(val);
                }
            }
        }
    })
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "added": body.path })))
}

/// POST /api/workspaces/remove — remove a workspace path (cannot remove built-in).
pub async fn remove_workspace_handler(
    Json(body): Json<WorkspacePathBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let builtin = config::builtin_workspaces_dir();
    if std::path::PathBuf::from(&body.path) == builtin {
        return Err((StatusCode::BAD_REQUEST, "Cannot remove the built-in workspace".into()));
    }
    config::update_settings_json(|root| {
        if let Some(arr) = root.get_mut("workspaces").and_then(|v| v.as_array_mut()) {
            arr.retain(|v| v.as_str() != Some(&body.path));
        }
        // If removing the default workspace, clear default_workspace
        if root.get("default_workspace").and_then(|v| v.as_str()) == Some(&body.path) {
            if let Some(obj) = root.as_object_mut() {
                obj.remove("default_workspace");
            }
        }
    })
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "removed": body.path })))
}

/// PUT /api/workspaces/default — set the default workspace.
pub async fn set_default_workspace_handler(
    Json(body): Json<WorkspacePathBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = std::path::PathBuf::from(&body.path);
    if !path.exists() || !path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, format!("Path does not exist: {}", body.path)));
    }
    config::update_settings_json(|root| {
        if let Some(obj) = root.as_object_mut() {
            if body.path.is_empty() {
                obj.remove("default_workspace");
            } else {
                obj.insert("default_workspace".into(), serde_json::json!(body.path));
            }
        }
    })
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "default_workspace": body.path })))
}

/// GET /api/previews — list all live preview sessions and the active tunnel URL.
pub async fn list_previews_handler(
    State(state): State<AppState>,
) -> Json<crate::api_types::PreviewsResponse> {
    let previews = common::previews::list_snapshots();
    let tunnel_url = state.tunnels.first_url();
    Json(crate::api_types::PreviewsResponse {
        previews,
        tunnel_url,
    })
}

/// DELETE /api/previews/:slug — close one preview and kill its dev-server port.
pub async fn delete_preview_handler(Path(slug): Path<String>) -> impl IntoResponse {
    if common::previews::delete_session(&slug) {
        (StatusCode::OK, format!("Preview {} closed", slug))
    } else {
        (StatusCode::NOT_FOUND, format!("Preview {} not found", slug))
    }
}

/// DELETE /api/sessions/:session_id — kill and remove a session.
pub async fn delete_session_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let uuid = match uuid::Uuid::parse_str(&session_id) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid session_id".to_string()),
    };
    let sid = SessionId(uuid);
    if state.pty_manager.delete_session(sid) {
        (StatusCode::OK, format!("Session {} deleted", session_id))
    } else {
        (StatusCode::NOT_FOUND, format!("Session {} not found", session_id))
    }
}
