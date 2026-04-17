//! Per-install-step executors: npm ACP agents, script-based agents,
//! channel plugins. Each function emits `running` → `done`/`error`
//! progress events and appends stdout/stderr to the onboarding log.

use std::sync::Arc;

use tauri::{AppHandle, Runtime};
use tokio::sync::Mutex;

use crate::onboarding::{plugin_install, InstallProgressEvent};

use super::util::{emit_progress, log_line};

pub(super) async fn install_npm_agent<R: Runtime>(
    app: &AppHandle<R>,
    agent_id: &str,
    agent_def: &common::resources::AgentDef,
    log_file: &Arc<Mutex<Option<std::fs::File>>>,
    had_error: &mut bool,
) {
    let task_id = format!("agent:{}:acp", agent_id);
    let Some(npm_pkg) = &agent_def.acp.npm_package else {
        return;
    };
    let bin_name = agent_def.acp.bin_name.as_deref().unwrap_or(npm_pkg);

    if common::env::resolve_acp_agent_bin(bin_name).is_ok() {
        emit_progress(
            app,
            &InstallProgressEvent {
                id: task_id,
                label: format!("{} — CLI install", agent_def.display_name),
                status: "skipped".into(),
                message: Some("Already installed".into()),
            },
        );
        log_line(
            log_file,
            &format!("[{}] ACP agent already installed, skipping", agent_id),
        );
        return;
    }

    let msg = format!("Running: npm install {}", npm_pkg);
    emit_progress(
        app,
        &InstallProgressEvent {
            id: task_id.clone(),
            label: format!("{} — CLI install", agent_def.display_name),
            status: "running".into(),
            message: Some(msg.clone()),
        },
    );
    log_line(log_file, &format!("[{}] {}", agent_id, msg));

    match common::agent_integrations::auto_install_npm_agent_with_output(npm_pkg).await {
        Ok(out) => {
            log_line(
                log_file,
                &format!("[{}] stdout:\n{}", agent_id, out.stdout),
            );
            log_line(
                log_file,
                &format!("[{}] stderr:\n{}", agent_id, out.stderr),
            );
            emit_progress(
                app,
                &InstallProgressEvent {
                    id: task_id,
                    label: format!("{} — CLI install", agent_def.display_name),
                    status: "done".into(),
                    message: None,
                },
            );
            log_line(log_file, &format!("[{}] npm install complete", agent_id));
        }
        Err(e) => {
            *had_error = true;
            let err_msg = format!("{:#}", e);
            emit_progress(
                app,
                &InstallProgressEvent {
                    id: task_id,
                    label: format!("{} — CLI install", agent_def.display_name),
                    status: "error".into(),
                    message: Some(err_msg.clone()),
                },
            );
            log_line(log_file, &format!("[{}] ERROR: {}", agent_id, err_msg));
        }
    }
}

pub(super) async fn install_script_agent<R: Runtime>(
    app: &AppHandle<R>,
    agent_id: &str,
    agent_def: &common::resources::AgentDef,
    log_file: &Arc<Mutex<Option<std::fs::File>>>,
    had_error: &mut bool,
) {
    let task_id = format!("agent:{}:acp", agent_id);

    if common::agent_integrations::is_program_available(&agent_def.acp.program) {
        emit_progress(
            app,
            &InstallProgressEvent {
                id: task_id,
                label: format!("{} — CLI install", agent_def.display_name),
                status: "skipped".into(),
                message: Some("Already installed".into()),
            },
        );
        log_line(
            log_file,
            &format!("[{}] CLI already available in PATH, skipping", agent_id),
        );
        return;
    }

    let Some(install_cmd) = &agent_def.acp.install_cmd else {
        return;
    };
    let msg = format!("Running: {}", install_cmd);
    emit_progress(
        app,
        &InstallProgressEvent {
            id: task_id.clone(),
            label: format!("{} — CLI install", agent_def.display_name),
            status: "running".into(),
            message: Some(msg.clone()),
        },
    );
    log_line(log_file, &format!("[{}] {}", agent_id, msg));

    match common::agent_integrations::auto_install_agent_cmd_with_output(install_cmd, agent_id)
        .await
    {
        Ok(out) => {
            log_line(
                log_file,
                &format!("[{}] stdout:\n{}", agent_id, out.stdout),
            );
            log_line(
                log_file,
                &format!("[{}] stderr:\n{}", agent_id, out.stderr),
            );
            emit_progress(
                app,
                &InstallProgressEvent {
                    id: task_id,
                    label: format!("{} — CLI install", agent_def.display_name),
                    status: "done".into(),
                    message: None,
                },
            );
            log_line(log_file, &format!("[{}] script install complete", agent_id));
        }
        Err(e) => {
            *had_error = true;
            let err_msg = format!("{:#}", e);
            emit_progress(
                app,
                &InstallProgressEvent {
                    id: task_id,
                    label: format!("{} — CLI install", agent_def.display_name),
                    status: "error".into(),
                    message: Some(err_msg.clone()),
                },
            );
            log_line(log_file, &format!("[{}] ERROR: {}", agent_id, err_msg));
        }
    }
}

pub(super) async fn install_channel_plugin<R: Runtime>(
    app: &AppHandle<R>,
    channel_id: &str,
    log_file: &Arc<Mutex<Option<std::fs::File>>>,
    had_error: &mut bool,
) {
    let task_id = format!("plugin:{}", channel_id);
    let plugin_def = common::resources::plugin_by_id(channel_id);
    let label = plugin_def
        .map(|p| p.name.clone())
        .unwrap_or_else(|| channel_id.to_string());

    // Check if already ready
    let status = plugin_install::check_plugin_status(channel_id.to_string());
    if status == "ready" {
        emit_progress(
            app,
            &InstallProgressEvent {
                id: task_id,
                label: format!("{} — Plugin install", label),
                status: "skipped".into(),
                message: Some("Already installed".into()),
            },
        );
        log_line(
            log_file,
            &format!("[plugin:{}] Already ready, skipping", channel_id),
        );
        return;
    }

    let github_url = match plugin_def {
        Some(p) => p.github.clone(),
        None => {
            emit_progress(
                app,
                &InstallProgressEvent {
                    id: task_id,
                    label: format!("{} — Plugin install", label),
                    status: "error".into(),
                    message: Some("Plugin not found in registry".into()),
                },
            );
            *had_error = true;
            return;
        }
    };

    emit_progress(
        app,
        &InstallProgressEvent {
            id: task_id.clone(),
            label: format!("{} — Plugin install", label),
            status: "running".into(),
            message: Some("Running: git clone + npm install + build".into()),
        },
    );
    log_line(
        log_file,
        &format!(
            "[plugin:{}] Starting install from {}",
            channel_id, github_url
        ),
    );

    let request = plugin_install::InstallPluginRequest {
        plugin_id: channel_id.to_string(),
        github_url,
    };
    match plugin_install::run_install_inner(request).await {
        Ok(resp) => {
            if resp.success {
                emit_progress(
                    app,
                    &InstallProgressEvent {
                        id: task_id,
                        label: format!("{} — Plugin install", label),
                        status: "done".into(),
                        message: None,
                    },
                );
                log_line(
                    log_file,
                    &format!("[plugin:{}] Install complete", channel_id),
                );
            } else {
                *had_error = true;
                emit_progress(
                    app,
                    &InstallProgressEvent {
                        id: task_id,
                        label: format!("{} — Plugin install", label),
                        status: "error".into(),
                        message: Some(resp.message),
                    },
                );
            }
        }
        Err(e) => {
            *had_error = true;
            let err_msg = format!("{:#}", e);
            emit_progress(
                app,
                &InstallProgressEvent {
                    id: task_id,
                    label: format!("{} — Plugin install", label),
                    status: "error".into(),
                    message: Some(err_msg.clone()),
                },
            );
            log_line(
                log_file,
                &format!("[plugin:{}] ERROR: {}", channel_id, err_msg),
            );
        }
    }
}
