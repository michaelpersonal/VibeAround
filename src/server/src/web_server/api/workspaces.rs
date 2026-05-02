use axum::{http::StatusCode, Json};

use common::config;

/// GET /api/workspaces -- list all workspaces.
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

/// POST /api/workspaces -- add a workspace path.
pub async fn add_workspace_handler(
    Json(body): Json<WorkspacePathBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = std::path::PathBuf::from(&body.path);
    if !path.exists() || !path.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Path does not exist or is not a directory: {}", body.path),
        ));
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

/// POST /api/workspaces/remove -- remove a workspace path (cannot remove built-in).
pub async fn remove_workspace_handler(
    Json(body): Json<WorkspacePathBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let builtin = config::builtin_workspaces_dir();
    if std::path::PathBuf::from(&body.path) == builtin {
        return Err((
            StatusCode::BAD_REQUEST,
            "Cannot remove the built-in workspace".into(),
        ));
    }
    config::update_settings_json(|root| {
        if let Some(arr) = root.get_mut("workspaces").and_then(|v| v.as_array_mut()) {
            arr.retain(|v| v.as_str() != Some(&body.path));
        }
        if root.get("default_workspace").and_then(|v| v.as_str()) == Some(&body.path) {
            if let Some(obj) = root.as_object_mut() {
                obj.remove("default_workspace");
            }
        }
    })
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "removed": body.path })))
}

/// PUT /api/workspaces/default -- set the default workspace.
pub async fn set_default_workspace_handler(
    Json(body): Json<WorkspacePathBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = std::path::PathBuf::from(&body.path);
    if !path.exists() || !path.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Path does not exist: {}", body.path),
        ));
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
