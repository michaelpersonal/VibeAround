//! Onboarding: first-run setup wizard.
//! Checks whether settings.json has `"onboarded": true`; exposes Tauri IPC
//! commands so the desktop-ui frontend can read/write settings and signal completion.

mod agent_integrations;
mod plugin_install;
mod plugin_session;

pub use plugin_install::{
    check_plugin_status, install_plugin,
    // Re-export Tauri macro-generated handler identifiers so generate_handler! works
    // when commands are referenced as `onboarding::install_plugin`.
    __cmd__install_plugin, __cmd__check_plugin_status,
};
pub use plugin_session::PluginSession;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::{Mutex, Notify};

use crate::{restart_daemon, OnboardingActive};
use common::config;
use common::plugins;

// ---------------------------------------------------------------------------
// Shared state types
// ---------------------------------------------------------------------------

pub struct OnboardingGate {
    pub notify: Arc<Notify>,
}

pub struct OnboardingSessions {
    pub plugin_sessions: Arc<Mutex<HashMap<String, PluginSession>>>,
}

// ---------------------------------------------------------------------------
// Settings helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Onboarding gate
// ---------------------------------------------------------------------------

/// Read current settings (exposed for startup integration sync).
pub fn get_settings_value() -> serde_json::Value {
    read_settings_value()
}

pub fn needs_onboarding() -> bool {
    let val = read_settings_value();
    !val.get("onboarded")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Resource summary types — expose agent/tunnel/plugin definitions to frontend
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub display_name: String,
    pub description: String,
}

#[derive(serde::Serialize)]
pub struct TunnelSummary {
    pub id: String,
    pub display_name: String,
}

#[derive(serde::Serialize)]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub github: String,
}

// ---------------------------------------------------------------------------
// Tauri commands — settings
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tauri commands — resource queries
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_agents() -> Vec<AgentSummary> {
    common::resources::AGENTS
        .iter()
        .map(|a| AgentSummary {
            id: a.id.clone(),
            display_name: a.display_name.clone(),
            description: a.description.clone(),
        })
        .collect()
}

#[tauri::command]
pub fn list_tunnels() -> Vec<TunnelSummary> {
    common::resources::TUNNELS
        .iter()
        .map(|t| TunnelSummary {
            id: t.id.clone(),
            display_name: t.display_name.clone(),
        })
        .collect()
}

#[tauri::command]
pub fn list_plugin_registry() -> Vec<PluginSummary> {
    common::resources::PLUGINS
        .iter()
        .map(|p| PluginSummary {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            github: p.github.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tauri commands — onboarding flow
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuthStartRequest {
    pub plugin_id: String,
    pub config: Value,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuthWaitRequest {
    pub plugin_id: String,
    pub params: Value,
}

#[derive(Debug, serde::Deserialize)]
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
        plugin_session::shutdown_plugin_session(&mut existing).await;
    }

    let mut session =
        plugin_session::spawn_auth_session(&request.plugin_id, request.config.clone())
            .await
            .map_err(|e| e.to_string())?;

    let result: Value =
        plugin_session::plugin_request(&mut session, "login_qr_start", request.config)
            .await
            .map_err(|e| e.to_string())?;

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

    let result: Value =
        plugin_session::plugin_request(session, "login_qr_wait", request.params)
            .await
            .map_err(|e| e.to_string())?;

    // Shutdown on success
    if result.get("connected").and_then(|v| v.as_bool()).unwrap_or(false) {
        if let Some(mut session) = sessions.remove(&request.plugin_id) {
            plugin_session::shutdown_plugin_session(&mut session).await;
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
        plugin_session::shutdown_plugin_session(&mut session).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn finish_onboarding<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, OnboardingSessions>,
    settings: Value,
) -> Result<(), String> {
    let mut sessions = state.plugin_sessions.lock().await;
    for (_, mut session) in sessions.drain() {
        plugin_session::shutdown_plugin_session(&mut session).await;
    }
    drop(sessions);

    let mut val = settings;
    if let Some(obj) = val.as_object_mut() {
        obj.insert("onboarded".into(), serde_json::json!(true));
    }
    write_settings_value(&val)?;

    // Install MCP config + skills into coding agents' global settings
    if let Err(e) = agent_integrations::install_agent_integrations(&val) {
        eprintln!("[onboarding] agent integration install failed (non-fatal): {:#}", e);
    }

    // Pre-install npm-based ACP agent packages (non-fatal)
    agent_integrations::install_acp_agents(&val).await;

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
