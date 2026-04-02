//! REST API handlers for the web server.
//!
//! - GET /api/sessions
//! - POST /api/sessions
//! - DELETE /api/sessions/:session_id
//! - GET /api/tmux/sessions
//! - GET /api/agents
//! - GET /api/services
//! - DELETE /api/services/:category/:id

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use common::config;
use common::pty::{list_tmux_sessions, tmux_available, PtyTool, SessionId};

use super::AppState;

/// GET /api/tmux/sessions — list active tmux sessions and whether tmux is available.
pub async fn list_tmux_sessions_handler() -> Json<serde_json::Value> {
    let available = tmux_available();
    let sessions = if available { list_tmux_sessions() } else { vec![] };
    Json(serde_json::json!({
        "available": available,
        "sessions": sessions,
    }))
}

/// GET /api/agents — list enabled agents and default agent for frontend agent selector.
pub async fn list_agents_handler() -> Json<serde_json::Value> {
    let cfg = config::ensure_loaded();
    let agents: Vec<serde_json::Value> = cfg.enabled_agents.iter().map(|kind| {
        serde_json::json!({
            "id": kind.to_string(),
            "name": kind.display_name(),
            "description": kind.description(),
        })
    }).collect();
    Json(serde_json::json!({
        "agents": agents,
        "default_agent": cfg.default_agent,
    }))
}

/// GET /api/services — list all services grouped by category.
pub async fn list_services_handler(State(state): State<AppState>) -> Json<common::service::StatusSnapshot> {
    Json(state.services.snapshot())
}

/// DELETE /api/services/:category/:id — kill a specific service.
pub async fn kill_service_handler(
    State(state): State<AppState>,
    Path((category, id)): Path<(String, String)>,
) -> impl IntoResponse {
    if state.services.kill_service(&category, &id) {
        (StatusCode::OK, format!("Killed {}/{}", category, id))
    } else {
        (StatusCode::NOT_FOUND, format!("Service {}/{} not found", category, id))
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
pub async fn list_sessions_handler(State(state): State<AppState>) -> Json<Vec<serde_json::Value>> {
    let items = state
        .pty_manager
        .list_sessions()
        .into_iter()
        .map(|item| serde_json::json!({
            "session_id": item.session_id,
            "tool": item.tool,
            "status": item.status,
            "created_at": item.created_at,
            "project_path": item.project_path,
            "tmux_session": item.tmux_session,
        }))
        .collect();
    Json(items)
}

/// POST /api/sessions — create a new PTY session.
pub async fn create_session_handler(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
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

    Ok(Json(serde_json::json!({
        "session_id": created.session_id,
        "tool": created.tool,
        "created_at": created.created_at,
        "project_path": created.project_path,
    })))
}

// ---------------------------------------------------------------------------
// Workspace management
// ---------------------------------------------------------------------------

/// GET /api/workspaces — list all workspaces.
pub async fn list_workspaces_handler() -> Json<serde_json::Value> {
    let cfg = config::ensure_loaded();
    let builtin = config::builtin_workspaces_dir();
    let all = cfg.all_workspaces();

    let items: Vec<serde_json::Value> = all
        .iter()
        .map(|ws| {
            let is_builtin = *ws == builtin;
            let is_default = cfg.default_workspace.as_ref() == Some(ws)
                || (cfg.default_workspace.is_none() && is_builtin);
            serde_json::json!({
                "path": ws.to_string_lossy(),
                "is_default": is_default,
                "is_builtin": is_builtin,
            })
        })
        .collect();

    Json(serde_json::json!({
        "workspaces": items,
        "default_workspace": cfg.default_workspace.as_ref().map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| builtin.to_string_lossy().to_string()),
    }))
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
        let arr = root
            .as_object_mut()
            .unwrap()
            .entry("workspaces")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .unwrap();
        let val = serde_json::Value::String(body.path.clone());
        if !arr.contains(&val) {
            arr.push(val);
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
            root.as_object_mut().unwrap().remove("default_workspace");
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
        let obj = root.as_object_mut().unwrap();
        if body.path.is_empty() {
            obj.remove("default_workspace");
        } else {
            obj.insert("default_workspace".into(), serde_json::json!(body.path));
        }
    })
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "default_workspace": body.path })))
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
