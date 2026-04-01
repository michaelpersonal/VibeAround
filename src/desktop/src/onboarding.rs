//! Onboarding: first-run setup wizard.
//! Checks whether settings.json has `"onboarded": true`; exposes Tauri IPC
//! commands so the desktop-ui frontend can read/write settings and signal completion.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, Notify};

use crate::{restart_daemon, OnboardingActive};
use common::config;
use common::plugins;

pub struct OnboardingGate {
    pub notify: Arc<Notify>,
}

pub struct OnboardingSessions {
    pub plugin_sessions: Arc<Mutex<HashMap<String, PluginSession>>>,
}

pub struct PluginSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_request_id: u64,
}

fn settings_path() -> PathBuf {
    config::data_dir().join("settings.json")
}

fn read_settings_value() -> Value {
    let path = settings_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_settings_value(val: &Value) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let pretty = serde_json::to_string_pretty(val).map_err(|e| e.to_string())?;
    std::fs::write(&path, pretty).map_err(|e| e.to_string())
}

/// Spawn a plugin's auth-standalone script (for onboarding QR/pairing flows).
/// Uses `dist/auth-standalone.js` which speaks raw JSON-RPC, not ACP.
async fn spawn_auth_session(name: &str, _config_value: Value) -> Result<PluginSession, String> {
    let plugin = plugins::find_plugin(name)
        .ok_or_else(|| format!("plugin '{}' not found or not built", name))?;
    let auth_entry = plugin.dir.join("dist").join("auth-standalone.js");
    if !auth_entry.exists() {
        return Err(format!(
            "auth script not found for plugin '{}' at {:?}",
            name, auth_entry
        ));
    }
    spawn_node_session(name, &auth_entry, &plugin.dir).await
}

/// Spawn a plugin's main entry point with ACP handshake (for runtime use).
async fn spawn_plugin_session(name: &str, config_value: Value) -> Result<PluginSession, String> {
    let plugin = plugins::find_plugin(name)
        .ok_or_else(|| format!("plugin '{}' not found or not built", name))?;
    let entry_point = plugin.entry_path();
    let plugin_dir = plugin.dir.clone();
    let mut session = spawn_node_session(name, &entry_point, &plugin_dir).await?;

    // ACP handshake: read the client's initialize request, respond with config
    let client_init_id: Value;
    loop {
        let mut line = String::new();
        let bytes = session.stdout.read_line(&mut line).await.map_err(|e| e.to_string())?;
        if bytes == 0 {
            return Err(format!("plugin '{}' exited before sending initialize", name));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        let msg: Value = serde_json::from_str(trimmed).map_err(|e| e.to_string())?;
        if msg.get("method").and_then(|v| v.as_str()) == Some("initialize") {
            client_init_id = msg.get("id").cloned().unwrap_or(Value::Null);
            break;
        }
    }

    let cache_dir = config::data_dir().join(".cache");
    let init_response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": client_init_id,
        "result": {
            "protocolVersion": "2025-03-26",
            "agentInfo": { "name": "vibearound-onboarding", "version": env!("CARGO_PKG_VERSION") },
            "_meta": {
                "config": config_value,
                "cacheDir": cache_dir.to_string_lossy(),
                "channelKind": name,
            }
        }
    });
    let line = serde_json::to_string(&init_response).map_err(|e| e.to_string())? + "\n";
    session.stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
    session.stdin.flush().await.map_err(|e| e.to_string())?;

    Ok(session)
}

/// Spawn a Node.js script and do a raw JSON-RPC initialize handshake.
/// Used for auth-standalone scripts that speak plain JSON-RPC (not ACP).
async fn spawn_node_session(name: &str, entry_point: &std::path::Path, plugin_dir: &std::path::Path) -> Result<PluginSession, String> {
    let mut child = Command::new("node")
        .arg(entry_point)
        .current_dir(plugin_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to spawn '{}': {}", name, e))?;

    let mut stdin = child.stdin.take().ok_or("stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("stdout unavailable")?;
    if let Some(stderr) = child.stderr.take() {
        let name = name.to_string();
        tauri::async_runtime::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[onboarding:{}] {}", name, line);
            }
        });
    }

    // Send raw JSON-RPC initialize and wait for response
    let init_req = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    });
    let line = serde_json::to_string(&init_req).map_err(|e| e.to_string())? + "\n";
    stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
    stdin.flush().await.map_err(|e| e.to_string())?;

    let mut stdout = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        let bytes = stdout.read_line(&mut line).await.map_err(|e| e.to_string())?;
        if bytes == 0 {
            return Err(format!("'{}' exited before initialize completed", name));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        let msg: Value = serde_json::from_str(trimmed).map_err(|e| e.to_string())?;
        if msg.get("id").and_then(|v| v.as_u64()) == Some(1) {
            if let Some(error) = msg.get("error") {
                return Err(error.get("message").and_then(|v| v.as_str()).unwrap_or("init error").to_string());
            }
            break;
        }
    }

    Ok(PluginSession {
        child,
        stdin,
        stdout,
        next_request_id: 2,
    })
}

async fn plugin_request<T: for<'de> Deserialize<'de>>(
    session: &mut PluginSession,
    method: &str,
    params: Value,
) -> Result<T, String> {
    let request_id = session.next_request_id;
    session.next_request_id += 1;

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    });
    let line = serde_json::to_string(&req).map_err(|e| e.to_string())? + "\n";
    session
        .stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("failed to write request '{}': {}", method, e))?;
    session.stdin.flush().await.map_err(|e| e.to_string())?;

    loop {
        let mut line = String::new();
        let bytes = session
            .stdout
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;
        if bytes == 0 {
            return Err(format!("plugin request '{}' ended without a response", method));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = serde_json::from_str(trimmed).map_err(|e| e.to_string())?;
        let id = msg.get("id").and_then(|v| v.as_u64());
        if id != Some(request_id) {
            continue;
        }
        if let Some(error) = msg.get("error") {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown plugin error");
            return Err(message.to_string());
        }
        let result = msg.get("result").cloned().unwrap_or(Value::Null);
        return serde_json::from_value::<T>(result).map_err(|e| e.to_string());
    }
}

async fn shutdown_plugin_session(session: &mut PluginSession) {
    let request_id = session.next_request_id;
    session.next_request_id += 1;
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "shutdown",
        "params": {}
    });
    if let Ok(line) = serde_json::to_string(&req) {
        let _ = session.stdin.write_all((line + "\n").as_bytes()).await;
        let _ = session.stdin.flush().await;
    }
    let _ = session.child.kill().await;
}

// ---------------------------------------------------------------------------
// Plugin install
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPluginRequest {
    pub plugin_id: String,
    pub github_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPluginResponse {
    pub success: bool,
    pub message: String,
    /// The plugin ID as declared in the installed plugin.json (may differ from the requested pluginId).
    pub actual_plugin_id: Option<String>,
}

#[tauri::command]
pub async fn install_plugin(request: InstallPluginRequest) -> Result<InstallPluginResponse, String> {
    let plugins_dir = config::data_dir().join("plugins");
    let target_dir = plugins_dir.join(&request.plugin_id);

    // Create plugins dir if needed
    std::fs::create_dir_all(&plugins_dir).map_err(|e| e.to_string())?;

    // Clone if not already present
    if !target_dir.exists() {
        eprintln!("[install_plugin] cloning {} → {:?}", request.github_url, target_dir);
        let output = tokio::process::Command::new("git")
            .args(["clone", "--depth", "1", &request.github_url, &target_dir.to_string_lossy()])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("git clone failed: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git clone failed: {}", stderr));
        }
    } else {
        eprintln!("[install_plugin] {} already exists, skipping clone", request.plugin_id);
    }

    // On Windows, npm ships as npm.cmd; use that so Command can find it.
    let npm = if cfg!(target_os = "windows") { "npm.cmd" } else { "npm" };

    // npm install
    eprintln!("[install_plugin] running npm install in {:?}", target_dir);
    let output = tokio::process::Command::new(npm)
        .args(["install"])
        .current_dir(&target_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("npm install failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("npm install failed: {}", stderr));
    }

    // npm run build
    eprintln!("[install_plugin] running npm run build in {:?}", target_dir);
    let output = tokio::process::Command::new(npm)
        .args(["run", "build"])
        .current_dir(&target_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("npm run build failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("npm run build failed: {}", stderr));
    }

    // Verify the plugin is discoverable after build
    let actual_id = match plugins::find_plugin(&request.plugin_id) {
        Some(p) => {
            eprintln!(
                "[install_plugin] {} installed and discoverable (manifest id='{}')",
                request.plugin_id, p.manifest.id
            );
            Some(p.manifest.id.clone())
        }
        None => {
            // Plugin dir exists but wasn't discovered — likely an ID mismatch or missing kind.
            // Try reading plugin.json directly to surface the actual id for the frontend.
            let manifest_path = target_dir.join("plugin.json");
            let fallback_id = std::fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(String::from));
            eprintln!(
                "[install_plugin] WARNING: {} built but not discoverable as channel plugin (manifest id={:?})",
                request.plugin_id,
                fallback_id
            );
            fallback_id
        }
    };

    Ok(InstallPluginResponse {
        success: true,
        message: format!("Plugin '{}' installed successfully", request.plugin_id),
        actual_plugin_id: actual_id,
    })
}

#[tauri::command]
pub fn check_plugin_status(plugin_id: String) -> String {
    let target_dir = config::data_dir().join("plugins").join(&plugin_id);
    if !target_dir.join("plugin.json").exists() {
        return "not_installed".to_string();
    }
    if !target_dir.join("dist").join("main.js").exists() {
        return "installed_not_built".to_string();
    }
    "ready".to_string()
}

// ---------------------------------------------------------------------------
// Generic plugin auth (QR login / pairing code)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuthStartRequest {
    pub plugin_id: String,
    pub config: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuthWaitRequest {
    pub plugin_id: String,
    pub params: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuthCancelRequest {
    pub plugin_id: String,
}

#[tauri::command]
pub async fn plugin_auth_start(
    state: State<'_, OnboardingSessions>,
    request: PluginAuthStartRequest,
) -> Result<Value, String> {
    let mut sessions = state.plugin_sessions.lock().await;
    if let Some(mut existing) = sessions.remove(&request.plugin_id) {
        shutdown_plugin_session(&mut existing).await;
    }

    let mut session = spawn_auth_session(&request.plugin_id, request.config.clone()).await?;

    // The auth script's start method name depends on the plugin.
    let method = "login_qr_start";
    let result: Value = plugin_request(&mut session, method, request.config).await?;

    sessions.insert(request.plugin_id, session);
    Ok(result)
}

#[tauri::command]
pub async fn plugin_auth_wait(
    state: State<'_, OnboardingSessions>,
    request: PluginAuthWaitRequest,
) -> Result<Value, String> {
    let mut sessions = state.plugin_sessions.lock().await;
    let session = sessions
        .get_mut(&request.plugin_id)
        .ok_or_else(|| format!("auth session for '{}' not started", request.plugin_id))?;

    let result: Value = plugin_request(session, "login_qr_wait", request.params).await?;

    // Shutdown on success
    if result.get("connected").and_then(|v| v.as_bool()).unwrap_or(false) {
        if let Some(mut session) = sessions.remove(&request.plugin_id) {
            shutdown_plugin_session(&mut session).await;
        }
    }

    Ok(result)
}

#[tauri::command]
pub async fn plugin_auth_cancel(
    state: State<'_, OnboardingSessions>,
    request: PluginAuthCancelRequest,
) -> Result<(), String> {
    let mut sessions = state.plugin_sessions.lock().await;
    if let Some(mut session) = sessions.remove(&request.plugin_id) {
        shutdown_plugin_session(&mut session).await;
    }
    Ok(())
}

pub fn needs_onboarding() -> bool {
    let val = read_settings_value();
    !val.get("onboarded")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[tauri::command]
pub fn get_settings() -> Result<Value, String> {
    Ok(read_settings_value())
}

#[tauri::command]
pub fn list_channel_plugins() -> Result<Vec<plugins::DiscoveredPluginSummary>, String> {
    Ok(plugins::list_channel_plugin_summaries())
}

#[tauri::command]
pub fn save_settings(settings: Value) -> Result<(), String> {
    write_settings_value(&settings)
}

#[tauri::command]
pub async fn finish_onboarding<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, OnboardingSessions>,
    settings: Value,
) -> Result<(), String> {
    let mut sessions = state.plugin_sessions.lock().await;
    for (_, mut session) in sessions.drain() {
        shutdown_plugin_session(&mut session).await;
    }
    drop(sessions);

    let mut val = settings;
    if let Some(obj) = val.as_object_mut() {
        obj.insert("onboarded".into(), serde_json::json!(true));
    }
    write_settings_value(&val)?;

    let _ = app.emit("onboarding-complete", ());

    if let Some(active) = app.try_state::<OnboardingActive>() {
        let was_onboarding = active
            .0
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        if was_onboarding {
            if let Some(gate) = app.try_state::<OnboardingGate>() {
                gate.notify.notify_one();
            }
        } else {
            restart_daemon(&app).await?;
        }
    }

    Ok(())
}
