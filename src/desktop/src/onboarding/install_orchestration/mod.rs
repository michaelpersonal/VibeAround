//! Install orchestration: runs agents + plugin installs with progress reporting.
//!
//! ## Module layout
//!
//! - [`util`]   — progress-event emission + log-file append + enabled-agent
//!                resolution helpers.
//! - [`steps`]  — per-step executors (`install_npm_agent`,
//!                `install_script_agent`, `install_channel_plugin`).
//!
//! The top-level `get_install_manifest`, `start`, and `cancel` functions
//! live in this file and drive `run_install` — the background task that
//! sweeps MCP/skill sync, then iterates enabled agents and channels.

mod steps;
mod util;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Mutex;

use common::config;

use crate::onboarding::{
    write_settings_value, InstallProgressEvent, InstallTaskInfo, OnboardingInstallState,
};

use steps::{install_channel_plugin, install_npm_agent, install_script_agent};
use util::{emit_progress, log_line, resolve_enabled_agents};

/// Returns the list of install tasks for the given settings, so the frontend
/// can pre-populate the progress list before install starts.
pub fn get_install_manifest(settings: &Value) -> Vec<InstallTaskInfo> {
    let all_agents = common::resources::agent_ids();
    let enabled_agents = resolve_enabled_agents(settings, &all_agents);

    let mut tasks = Vec::new();

    for agent_id in &enabled_agents {
        let agent_def = match common::resources::agent_by_id(agent_id) {
            Some(def) => def,
            None => continue,
        };

        // MCP config + skill are always installed
        if agent_def.global_config.is_some() {
            tasks.push(InstallTaskInfo {
                id: format!("agent:{}:mcp", agent_id),
                label: format!("{} — MCP config", agent_def.display_name),
            });
            if agent_def
                .global_config
                .as_ref()
                .and_then(|c| c.skill_dir.as_ref())
                .is_some()
            {
                tasks.push(InstallTaskInfo {
                    id: format!("agent:{}:skill", agent_id),
                    label: format!("{} — Skill file", agent_def.display_name),
                });
            }
        }

        // ACP agent install (npm or script) — only for installable types
        let install_type = agent_def.install.as_ref().map(|i| i.install_type.as_str());
        if matches!(install_type, Some("npm") | Some("script")) {
            tasks.push(InstallTaskInfo {
                id: format!("agent:{}:acp", agent_id),
                label: format!("{} — CLI install", agent_def.display_name),
            });
        }
    }

    // Channel plugins
    let enabled_channels = settings
        .get("channels")
        .and_then(|v| v.as_object())
        .map(|obj| obj.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    for channel_id in &enabled_channels {
        let plugin_def = common::resources::plugin_by_id(channel_id);
        let label = plugin_def
            .map(|p| p.name.clone())
            .unwrap_or_else(|| channel_id.clone());
        tasks.push(InstallTaskInfo {
            id: format!("plugin:{}", channel_id),
            label: format!("{} — Plugin install", label),
        });
    }

    tasks
}

/// Start the install, saving settings first, then spawning the background task.
pub async fn start<R: Runtime>(
    app: AppHandle<R>,
    install_state: &OnboardingInstallState,
    settings: Value,
) -> Result<(), String> {
    install_state.cancelled.store(false, Ordering::Relaxed);

    // Save settings with onboarded: true
    let mut val = settings;
    if let Some(obj) = val.as_object_mut() {
        obj.insert("onboarded".into(), serde_json::json!(true));
    }
    write_settings_value(&val)?;

    // Create log file
    let log_dir = config::data_dir().join("logs").join("onboarding");
    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let log_path = log_dir.join(format!("{}.log", timestamp));
    let log_file = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
    {
        let mut lf = install_state.log_file.lock().await;
        *lf = Some(log_file);
    }

    let cancelled = Arc::clone(&install_state.cancelled);
    let child_proc = Arc::clone(&install_state.child_process);
    let log_file_arc = Arc::clone(&install_state.log_file);

    tauri::async_runtime::spawn(async move {
        run_install(app, val, cancelled, child_proc, log_file_arc).await;
    });

    Ok(())
}

/// Cancel a running install.
pub async fn cancel(install_state: &OnboardingInstallState) -> Result<(), String> {
    install_state.cancelled.store(true, Ordering::Relaxed);
    let mut child = install_state.child_process.lock().await;
    if let Some(ref mut proc) = *child {
        let _ = proc.kill().await;
    }
    *child = None;
    Ok(())
}

async fn run_install<R: Runtime>(
    app: AppHandle<R>,
    settings: Value,
    cancelled: Arc<AtomicBool>,
    _child_proc: Arc<Mutex<Option<tokio::process::Child>>>,
    log_file: Arc<Mutex<Option<std::fs::File>>>,
) {
    let all_agents = common::resources::agent_ids();
    let enabled_agents = resolve_enabled_agents(&settings, &all_agents);
    let mut had_error = false;

    // Install MCP config + skill files for all enabled agents in one global
    // sweep BEFORE the per-agent loop.
    if !enabled_agents.is_empty() {
        log_line(
            &log_file,
            "[onboarding] Running sync_integrations (global MCP + skills sweep)",
        );
        common::agent_integrations::sync_integrations(&settings);
    }

    // --- Agent installs ---
    for agent_id in &enabled_agents {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        let agent_def = match common::resources::agent_by_id(agent_id) {
            Some(def) => def,
            None => continue,
        };

        // MCP config
        if agent_def.global_config.is_some() {
            let task_id = format!("agent:{}:mcp", agent_id);
            emit_progress(
                &app,
                &InstallProgressEvent {
                    id: task_id.clone(),
                    label: format!("{} — MCP config", agent_def.display_name),
                    status: "running".into(),
                    message: Some("Installing MCP config…".into()),
                },
            );
            log_line(&log_file, &format!("[{}] Installing MCP config", agent_id));

            emit_progress(
                &app,
                &InstallProgressEvent {
                    id: task_id,
                    label: format!("{} — MCP config", agent_def.display_name),
                    status: "done".into(),
                    message: None,
                },
            );

            if agent_def
                .global_config
                .as_ref()
                .and_then(|c| c.skill_dir.as_ref())
                .is_some()
            {
                let skill_id = format!("agent:{}:skill", agent_id);
                emit_progress(
                    &app,
                    &InstallProgressEvent {
                        id: skill_id,
                        label: format!("{} — Skill file", agent_def.display_name),
                        status: "done".into(),
                        message: None,
                    },
                );
            }
        }

        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        // ACP agent install (npm or script)
        let install_type = agent_def.install.as_ref().map(|i| i.install_type.as_str());
        match install_type {
            Some("npm") => install_npm_agent(&app, agent_id, agent_def, &log_file, &mut had_error).await,
            Some("script") => {
                install_script_agent(&app, agent_id, agent_def, &log_file, &mut had_error).await
            }
            _ => {} // "path" type — nothing to install
        }
    }

    // --- Channel plugin installs ---
    let enabled_channels = settings
        .get("channels")
        .and_then(|v| v.as_object())
        .map(|obj| obj.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    for channel_id in &enabled_channels {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        install_channel_plugin(&app, channel_id, &log_file, &mut had_error).await;
    }

    // Emit final complete event
    let final_status = if cancelled.load(Ordering::Relaxed) {
        "cancelled"
    } else if had_error {
        "error"
    } else {
        "complete"
    };

    let _ = app.emit(
        "onboarding-install-complete",
        serde_json::json!({
            "status": final_status,
        }),
    );

    let mut lf = log_file.lock().await;
    *lf = None;
}
