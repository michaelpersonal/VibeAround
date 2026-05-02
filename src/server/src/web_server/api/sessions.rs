use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use common::pty::{list_tmux_sessions, tmux_available, PtyTool, SessionId};

use crate::web_server::AppState;

/// GET /api/tmux/sessions -- list active tmux sessions and whether tmux is available.
pub async fn list_tmux_sessions_handler() -> Json<crate::api_types::TmuxSessionsResponse> {
    let available = tmux_available();
    let sessions = if available {
        list_tmux_sessions()
    } else {
        vec![]
    };
    Json(crate::api_types::TmuxSessionsResponse {
        available,
        sessions,
    })
}

/// Request body for POST /api/sessions.
#[derive(serde::Deserialize)]
pub(crate) struct CreateSessionBody {
    tool: Option<PtyTool>,
    profile_id: Option<String>,
    launch_target: Option<String>,
    project_path: Option<String>,
    tmux_session: Option<String>,
    theme: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

/// GET /api/sessions -- list all active sessions.
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
            profile_id: item.profile_id,
            profile_label: item.profile_label,
            launch_target: item.launch_target,
            tmux_session: item.tmux_session,
        })
        .collect();
    Json(items)
}

/// POST /api/sessions -- create a new PTY session.
pub async fn create_session_handler(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionBody>,
) -> Result<Json<crate::api_types::CreateSessionResponse>, (StatusCode, String)> {
    let initial_size = match (body.cols, body.rows) {
        (Some(c), Some(r)) => Some((c, r)),
        _ => None,
    };

    let created = match (body.profile_id.as_deref(), body.launch_target.as_deref()) {
        (Some(profile_id), Some(launch_target)) => {
            if body.tool.is_some() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "profile sessions cannot also specify tool".to_string(),
                ));
            }
            if body.tmux_session.is_some() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "profile sessions cannot attach tmux".to_string(),
                ));
            }
            let profile = common::profiles::schema::load(profile_id)
                .map(common::profiles::normalize_legacy_profile)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("profile '{}' not found", profile_id),
                    )
                })?;
            if !common::profiles::runtime::launch_targets_for_api_types(&profile.api_types)
                .iter()
                .any(|(target, _, _)| *target == launch_target)
            {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("profile '{}' cannot launch '{}'", profile.id, launch_target),
                ));
            }
            let rendered = common::profiles::runtime::render_for_launch(&profile, launch_target)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            let command_args = rendered.command_args.clone();
            let env = common::profiles::runtime::materialize_env(&profile.id, rendered)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let agent_id = common::profiles::runtime::agent_id_for(launch_target)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            let agent = common::resources::agent_by_id(agent_id).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("agent '{}' not found", agent_id),
                )
            })?;
            let pty_tool = PtyTool::from_agent_id(agent_id).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("agent '{}' cannot be launched in a PTY", agent_id),
                )
            })?;
            state
                .pty_manager
                .create_profile_session(
                    pty_tool,
                    command_with_args(&agent.pty.command, &command_args),
                    env,
                    profile.id.clone(),
                    profile.label.clone(),
                    launch_target.to_string(),
                    body.project_path.clone(),
                    body.theme.clone(),
                    initial_size,
                )
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        }
        (None, None) => {
            let tool = body.tool.ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "missing tool for direct session".to_string(),
                )
            })?;
            if body.tmux_session.is_some() && tool != PtyTool::Generic {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "tmux sessions must use the generic tool".to_string(),
                ));
            }
            state
                .pty_manager
                .create_session(
                    tool,
                    body.project_path.clone(),
                    body.tmux_session.clone(),
                    body.theme.clone(),
                    initial_size,
                )
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "profile_id and launch_target must be provided together".to_string(),
            ));
        }
    };

    Ok(Json(crate::api_types::CreateSessionResponse {
        session_id: created.session_id,
        tool: created.tool,
        created_at: created.created_at,
        project_path: created.project_path,
        profile_id: created.profile_id,
        profile_label: created.profile_label,
        launch_target: created.launch_target,
    }))
}

/// DELETE /api/sessions/:session_id -- kill and remove a session.
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
        (
            StatusCode::NOT_FOUND,
            format!("Session {} not found", session_id),
        )
    }
}

fn command_with_args(command: &str, args: &[String]) -> String {
    if args.is_empty() {
        return command.to_string();
    }
    let mut out = command.to_string();
    for arg in args {
        out.push(' ');
        out.push_str(&shell_quote(arg));
    }
    out
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
