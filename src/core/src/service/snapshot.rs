//! Snapshot types serialized to Dashboard API / WebSocket clients.

use serde::Serialize;

use super::status::ServiceStatus;

/// Web server metadata (read-only).
#[derive(Debug, Clone, Serialize)]
pub struct ServerMeta {
    pub started_at: u64,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusSnapshot {
    pub server: ServerMeta,
    pub tunnels: Vec<ServiceInfo>,
    pub agents: Vec<ServiceInfo>,
    pub channels: Vec<ServiceInfo>,
    pub pty_session_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub id: String,
    pub name: String,
    pub status: String,
    pub uptime_secs: u64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn status_string(s: &ServiceStatus) -> String {
    match s {
        ServiceStatus::Running => "running".into(),
        ServiceStatus::Stopped { reason } => format!("stopped: {}", reason),
        ServiceStatus::Failed { error } => format!("failed: {}", error),
    }
}

pub(super) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
