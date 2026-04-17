//! Shared helpers for install orchestration: progress emission, log
//! line append, and enabled-agent resolution.

use std::io::Write as _;
use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Mutex;

use crate::onboarding::InstallProgressEvent;

pub(super) fn emit_progress<R: Runtime>(app: &AppHandle<R>, event: &InstallProgressEvent) {
    let _ = app.emit("onboarding-install-progress", event);
}

pub(super) fn log_line(log_file: &Arc<Mutex<Option<std::fs::File>>>, line: &str) {
    if let Ok(mut guard) = log_file.try_lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{}", line);
        }
    }
}

pub(super) fn resolve_enabled_agents(settings: &Value, all_agents: &[&str]) -> Vec<String> {
    common::agent_integrations::resolve_enabled_agents(settings, all_agents)
}
