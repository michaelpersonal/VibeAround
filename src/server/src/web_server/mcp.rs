//! MCP Streamable HTTP endpoint — POST /mcp
//!
//! Implements a JSON-RPC 2.0 server for the Model Context Protocol.
//! Methods: initialize, notifications/initialized, tools/list, tools/call.
//!
//! MCP tools are **stateless** — they validate inputs and return text.
//! They never touch ACPHub, pods, or bridges. Session loading happens
//! later when the user sends `/pickup` in the IM channel.

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use super::AppState;

/// JSON-RPC 2.0 request envelope.
#[derive(serde::Deserialize)]
pub struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

/// Build a JSON-RPC 2.0 success response.
fn jsonrpc_ok(id: Option<serde_json::Value>, result: serde_json::Value) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
}

/// Build a JSON-RPC 2.0 error response.
fn jsonrpc_err(id: Option<serde_json::Value>, code: i64, message: &str) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    }))
}

fn mcp_text(id: Option<serde_json::Value>, text: &str) -> Json<serde_json::Value> {
    jsonrpc_ok(id, serde_json::json!({
        "content": [{ "type": "text", "text": text }]
    }))
}

fn mcp_error_text(id: Option<serde_json::Value>, text: &str) -> Json<serde_json::Value> {
    jsonrpc_ok(id, serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true
    }))
}

/// POST /mcp — MCP Streamable HTTP endpoint.
pub async fn mcp_handler(
    State(state): State<AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> axum::response::Response {
    if req.jsonrpc != "2.0" {
        return jsonrpc_err(req.id, -32600, "Invalid JSON-RPC version").into_response();
    }

    // Notifications (no id) must return 202 Accepted with no body per MCP spec.
    if req.method.starts_with("notifications/") {
        return StatusCode::ACCEPTED.into_response();
    }

    match req.method.as_str() {
        "initialize" => mcp_initialize(req.id).into_response(),
        "tools/list" => mcp_tools_list(req.id).into_response(),
        "tools/call" => mcp_tools_call(req.id, req.params, &state).await.into_response(),
        _ => jsonrpc_err(req.id, -32601, &format!("Method not found: {}", req.method)).into_response(),
    }
}

fn mcp_initialize(id: Option<serde_json::Value>) -> Json<serde_json::Value> {
    jsonrpc_ok(id, serde_json::json!({
        "protocolVersion": "2025-03-26",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "vibearound", "version": "0.1.0" }
    }))
}

fn mcp_tools_list(id: Option<serde_json::Value>) -> Json<serde_json::Value> {
    jsonrpc_ok(id, common::resources::mcp_tools_list_json())
}

async fn mcp_tools_call(
    id: Option<serde_json::Value>,
    params: Option<serde_json::Value>,
    state: &AppState,
) -> Json<serde_json::Value> {
    let params = match params {
        Some(p) => p,
        None => return jsonrpc_err(id, -32602, "Missing params"),
    };

    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let arguments = match params.get("arguments") {
        Some(a) => a,
        None => return jsonrpc_err(id, -32602, "Missing arguments"),
    };

    match tool_name {
        "prepare_handover" => mcp_prepare_handover(id, arguments).await,
        "register_workspace" => mcp_register_workspace(id, arguments).await,
        "dispatch_task" => mcp_dispatch_task(id, arguments, state).await,
        _ => jsonrpc_err(id, -32602, &format!("Unknown tool: {}", tool_name)),
    }
}

// ---------------------------------------------------------------------------
// prepare_handover — stateless, no ACPHub dependency
// ---------------------------------------------------------------------------

async fn mcp_prepare_handover(
    id: Option<serde_json::Value>,
    arguments: &serde_json::Value,
) -> Json<serde_json::Value> {
    let cwd = match arguments.get("cwd").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return jsonrpc_err(id, -32602, "Missing required argument: cwd"),
    };
    let session_id_arg = arguments.get("session_id").and_then(|v| v.as_str()).map(String::from);
    let agent_kind = match arguments.get("agent_kind").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return jsonrpc_err(id, -32602, "Missing required argument: agent_kind"),
    };
    let agent_kind_str = agent_kind;

    // Validate cwd is a known workspace.
    // Built-in workspaces under ~/.vibearound/workspaces/ are always accepted.
    let config = common::config::ensure_loaded();
    let cwd_path = std::path::PathBuf::from(cwd);
    let builtin_dir = common::config::builtin_workspaces_dir();
    let is_builtin = cwd_path.starts_with(&builtin_dir);
    let is_registered = config
        .all_workspaces()
        .iter()
        .any(|ws| ws == &cwd_path);

    if !is_builtin && !is_registered {
        return mcp_error_text(id, &format!(
            "Workspace {} is not registered in VibeAround.\n\
             Use the `register_workspace` tool to add it first, then retry.",
            cwd
        ));
    }

    // Resolve session ID: use provided value, or auto-discover from session files
    let session_id = match session_id_arg {
        Some(sid) if !sid.is_empty() => sid,
        _ => {
            match find_latest_session(agent_kind_str, &cwd_path) {
                Some(sid) => sid,
                None => {
                    let hint = match agent_kind_str {
                        "claude" => "In Claude Code, you can find it by running /status.",
                        "gemini" => "In Gemini CLI, run /resume to browse recent sessions.",
                        "codex" => "In Codex CLI, run `codex resume` to see recent sessions.",
                        _ => "Check your agent's session history.",
                    };
                    return mcp_error_text(id, &format!(
                        "Could not auto-discover session ID. Please provide your session_id explicitly.\n{}",
                        hint
                    ));
                }
            }
        }
    };

    let code = common::pickup_codes::store(
        agent_kind_str.to_string(),
        session_id,
        cwd.to_string(),
    );
    let pickup_cmd = format!("/pickup {}", code);
    mcp_text(id, &format!(
        "Handover prepared.\n\n\
         Tell the user to send this command in any IM chat connected to VibeAround:\n\
         {}\n\n\
         The code expires in 2 minutes. After sending the command, the user's next message will resume this session.",
        pickup_cmd
    ))
}

// ---------------------------------------------------------------------------
// register_workspace — writes to VibeAround settings.json
// ---------------------------------------------------------------------------

async fn mcp_register_workspace(
    id: Option<serde_json::Value>,
    arguments: &serde_json::Value,
) -> Json<serde_json::Value> {
    let cwd = match arguments.get("cwd").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return jsonrpc_err(id, -32602, "Missing required argument: cwd"),
    };

    let cwd_path = std::path::PathBuf::from(cwd);
    if !cwd_path.is_dir() {
        return mcp_error_text(id, &format!(
            "Directory does not exist: {}",
            cwd
        ));
    }

    // Check if already registered
    let config = common::config::ensure_loaded();
    let already_registered = config
        .all_workspaces()
        .iter()
        .any(|ws| ws == &cwd_path);

    if already_registered {
        return mcp_text(id, &format!(
            "Workspace {} is already registered.",
            cwd
        ));
    }

    // Add to settings.json
    let cwd_owned = cwd.to_string();
    if let Err(e) = common::config::update_settings_json(move |settings| {
        if let Some(obj) = settings.as_object_mut() {
            let workspaces = obj
                .entry("workspaces")
                .or_insert_with(|| serde_json::json!([]));
            if let Some(arr) = workspaces.as_array_mut() {
                arr.push(serde_json::Value::String(cwd_owned));
            }
        }
    }) {
        return mcp_error_text(id, &format!(
            "Failed to update settings: {}",
            e
        ));
    }

    mcp_text(id, &format!(
        "Workspace {} registered successfully.",
        cwd
    ))
}

// ---------------------------------------------------------------------------
// dispatch_task — existing tool (TODO: migrate to AgentManager)
// ---------------------------------------------------------------------------

async fn mcp_dispatch_task(
    id: Option<serde_json::Value>,
    arguments: &serde_json::Value,
    _state: &AppState,
) -> Json<serde_json::Value> {
    let workspace = match arguments.get("workspace").and_then(|v| v.as_str()) {
        Some(w) => std::path::PathBuf::from(w),
        None => return jsonrpc_err(id, -32602, "Missing required argument: workspace"),
    };

    let data_dir = common::config::data_dir();
    if workspace == data_dir || workspace == data_dir.join("") {
        return mcp_error_text(id, &format!(
            "Error: workspace must be a project-specific directory under {}/workspaces/<name>/.",
            data_dir.display()
        ));
    }

    let _message = match arguments.get("message").and_then(|v| v.as_str()) {
        Some(m) => m,
        None => return jsonrpc_err(id, -32602, "Missing required argument: message"),
    };

    // TODO: migrate to AgentManager
    mcp_error_text(id, "MCP dispatch_task is not yet available in the new hub architecture")
}

// ---------------------------------------------------------------------------
// Session auto-discovery — find the most recent session file for a workspace
// ---------------------------------------------------------------------------

/// Find the most recent session ID for a given agent kind and workspace.
fn find_latest_session(agent_kind: &str, cwd: &std::path::Path) -> Option<String> {
    match agent_kind {
        "claude" => find_latest_claude_session(cwd),
        "gemini" => find_latest_gemini_session(cwd),
        "codex" => find_latest_codex_session(cwd),
        _ => None,
    }
}

/// Find the most recent Claude session file for a given cwd.
/// Claude encodes the cwd by replacing non-alphanumeric chars with `-`.
fn find_latest_claude_session(cwd: &std::path::Path) -> Option<String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let projects_dir = std::path::PathBuf::from(home).join(".claude").join("projects");

    // Claude encodes cwd: /Users/foo/bar → -Users-foo-bar
    let encoded_cwd = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>();

    let session_dir = projects_dir.join(&encoded_cwd);
    if !session_dir.is_dir() {
        return None;
    }

    // Find the .jsonl file with the most recent modification time
    let mut best: Option<(std::time::SystemTime, String)> = None;
    if let Ok(entries) = std::fs::read_dir(&session_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(meta) = path.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };

            match &best {
                Some((best_time, _)) if modified <= *best_time => {}
                _ => {
                    best = Some((modified, stem.to_string()));
                }
            }
        }
    }

    best.map(|(_, session_id)| session_id)
}

/// Find the most recent Gemini session for a given cwd.
/// Gemini maps cwd → slug via `~/.gemini/projects.json`, then stores
/// session files at `~/.gemini/tmp/<slug>/chats/session-*.json`.
fn find_latest_gemini_session(cwd: &std::path::Path) -> Option<String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let gemini_dir = std::path::PathBuf::from(&home).join(".gemini");

    // Read projects.json to map cwd → slug
    let projects_path = gemini_dir.join("projects.json");
    let projects_data = std::fs::read_to_string(&projects_path).ok()?;
    let projects_json: serde_json::Value = serde_json::from_str(&projects_data).ok()?;
    let projects_map = projects_json.get("projects")?.as_object()?;

    let cwd_str = cwd.to_string_lossy();
    let slug = projects_map.get(cwd_str.as_ref())?.as_str()?;

    let chats_dir = gemini_dir.join("tmp").join(slug).join("chats");
    if !chats_dir.is_dir() {
        return None;
    }

    // Find the most recent session-*.json file
    let mut best: Option<(std::time::SystemTime, String)> = None;
    if let Ok(entries) = std::fs::read_dir(&chats_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !fname.starts_with("session-") || path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(meta) = path.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };

            match &best {
                Some((best_time, _)) if modified <= *best_time => {}
                _ => {
                    // Parse JSON to extract sessionId
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(sid) = json.get("sessionId").and_then(|v| v.as_str()) {
                                best = Some((modified, sid.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }

    best.map(|(_, session_id)| session_id)
}

/// Find the most recent Codex session for a given cwd.
/// Codex stores sessions at `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
/// The first line of each file contains `payload.cwd` and `payload.id`.
fn find_latest_codex_session(cwd: &std::path::Path) -> Option<String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let sessions_dir = std::path::PathBuf::from(&home).join(".codex").join("sessions");
    if !sessions_dir.is_dir() {
        return None;
    }

    let cwd_str = cwd.to_string_lossy();
    let mut best: Option<(std::time::SystemTime, String)> = None;

    // Walk the sessions directory recursively
    fn walk_codex_sessions(
        dir: &std::path::Path,
        cwd_str: &str,
        best: &mut Option<(std::time::SystemTime, String)>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_codex_sessions(&path, cwd_str, best);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = path.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };

            // Skip if older than current best
            if let Some((best_time, _)) = best {
                if modified <= *best_time {
                    continue;
                }
            }

            // Read first line and check cwd match
            let Ok(file) = std::fs::File::open(&path) else { continue };
            let reader = std::io::BufRead::lines(std::io::BufReader::new(file));
            let Some(Ok(first_line)) = reader.into_iter().next() else { continue };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&first_line) else { continue };

            let payload = match json.get("payload") {
                Some(p) => p,
                None => continue,
            };
            let session_cwd = match payload.get("cwd").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => continue,
            };
            if session_cwd != cwd_str {
                continue;
            }
            if let Some(sid) = payload.get("id").and_then(|v| v.as_str()) {
                *best = Some((modified, sid.to_string()));
            }
        }
    }

    walk_codex_sessions(&sessions_dir, &cwd_str, &mut best);
    best.map(|(_, session_id)| session_id)
}
