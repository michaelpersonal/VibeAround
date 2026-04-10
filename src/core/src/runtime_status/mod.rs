use std::sync::Arc;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::acp_hub::event::SystemEvent;
use crate::acp_hub::pod::PodSnapshot;
use crate::service::ServiceInfo;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAgentStatus {
    pub id: String,
    pub route_key: String,
    pub channel_kind: String,
    pub chat_id: String,
    pub cli_kind: Option<String>,
    pub profile: Option<String>,
    pub session_id: Option<String>,
    pub busy: bool,
    pub failed: Option<String>,
    pub started_at: u64,
    pub initialize: Option<agent_client_protocol::InitializeResponse>,
}

pub struct RuntimeStatusStore {
    runtimes: dashmap::DashMap<String, RuntimeAgentStatus>,
    change_tx: broadcast::Sender<()>,
}

impl RuntimeStatusStore {
    pub fn new(change_tx: broadcast::Sender<()>) -> Arc<Self> {
        Arc::new(Self {
            runtimes: dashmap::DashMap::new(),
            change_tx,
        })
    }

    pub fn project_event(&self, event: &SystemEvent) {
        match event {
            SystemEvent::RouteCreated { route } => {
                let snapshot = PodSnapshot {
                    route: route.clone(),
                    bot_identity: None,
                    session_id: None,
                    cli_kind: None,
                    profile: None,
                    workspace: None,
                    busy: false,
                    failed: None,
                    started_at: now_secs(),
                    initialize: None,
                };
                self.upsert(snapshot);
            }
            SystemEvent::RouteClosed { route, .. } => {
                self.runtimes.remove(&route.as_key());
                self.notify_change();
            }
            SystemEvent::RouteFailed { route, error }
            | SystemEvent::AgentInitializeFailed {
                route,
                error,
                ..
            } => {
                if let Some(mut entry) = self.runtimes.get_mut(&route.as_key()) {
                    entry.failed = Some(error.clone());
                }
                self.notify_change();
            }
            SystemEvent::AgentInitialized { route, initialize, .. } => {
                if let Some(mut entry) = self.runtimes.get_mut(&route.as_key()) {
                    entry.initialize = Some(initialize.clone());
                    entry.failed = None;
                }
                self.notify_change();
            }
            SystemEvent::SessionReady { route, session_id } => {
                if let Some(mut entry) = self.runtimes.get_mut(&route.as_key()) {
                    entry.session_id = Some(session_id.clone());
                }
                self.notify_change();
            }
            SystemEvent::SnapshotChanged { snapshot, .. } => {
                self.upsert(snapshot.clone());
            }
        }
    }

    pub fn snapshot_agents(&self) -> Vec<ServiceInfo> {
        self.runtimes
            .iter()
            .map(|entry| {
                let runtime = entry.value();
                let mut extra = serde_json::Map::new();
                extra.insert("routeKey".into(), runtime.route_key.clone().into());
                extra.insert("channelKind".into(), runtime.channel_kind.clone().into());
                extra.insert("chatId".into(), runtime.chat_id.clone().into());
                if let Some(kind) = &runtime.cli_kind {
                    extra.insert("kind".into(), kind.clone().into());
                }
                if let Some(profile) = &runtime.profile {
                    extra.insert("profile".into(), profile.clone().into());
                }
                if let Some(session_id) = &runtime.session_id {
                    extra.insert("sessionId".into(), session_id.clone().into());
                }
                extra.insert("busy".into(), runtime.busy.into());
                if let Some(failed) = &runtime.failed {
                    extra.insert("error".into(), failed.clone().into());
                }
                if let Some(initialize) = &runtime.initialize {
                    if let Ok(value) = serde_json::to_value(initialize) {
                        extra.insert("initialize".into(), value);
                    }
                    if let Some(agent_info) = &initialize.agent_info {
                        extra.insert("agentName".into(), agent_info.name.clone().into());
                        if let Some(title) = &agent_info.title {
                            extra.insert("agentTitle".into(), title.clone().into());
                        }
                        extra.insert("agentVersion".into(), agent_info.version.clone().into());
                    }
                    extra.insert(
                        "protocolVersion".into(),
                        format!("{:?}", initialize.protocol_version).into(),
                    );
                }

                ServiceInfo {
                    id: runtime.route_key.clone(),
                    name: format!(
                        "{} ({})",
                        runtime
                            .cli_kind
                            .clone()
                            .unwrap_or_else(|| "agent".to_string()),
                        runtime.id
                    ),
                    status: if runtime.failed.is_some() {
                        "failed".to_string()
                    } else {
                        "running".to_string()
                    },
                    uptime_secs: now_secs().saturating_sub(runtime.started_at),
                    extra,
                }
            })
            .collect()
    }

    fn upsert(&self, snapshot: PodSnapshot) {
        let route_key = snapshot.route.as_key();
        let service_key = snapshot.service_key();
        self.runtimes.insert(
            route_key.clone(),
            RuntimeAgentStatus {
                id: service_key,
                route_key,
                channel_kind: snapshot.route.channel_kind.clone(),
                chat_id: snapshot.route.chat_id.clone(),
                cli_kind: snapshot.cli_kind,
                profile: snapshot.profile,
                session_id: snapshot.session_id,
                busy: snapshot.busy,
                failed: snapshot.failed,
                started_at: snapshot.started_at,
                initialize: snapshot.initialize,
            },
        );
        self.notify_change();
    }

    fn notify_change(&self) {
        let _ = self.change_tx.send(());
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::routing::RouteKey;

    /// Build a snapshot with profile + cli_kind to produce a 4-segment service_key.
    fn snapshot_with_profile() -> PodSnapshot {
        PodSnapshot {
            route: RouteKey::new("telegram", "chat_42"),
            bot_identity: None,
            session_id: None,
            cli_kind: Some("claude".into()),
            profile: Some("work".into()),
            workspace: None,
            busy: false,
            failed: None,
            started_at: 100,
            initialize: None,
        }
    }

    #[test]
    fn published_id_is_route_key_not_service_key() {
        let (tx, _rx) = broadcast::channel(4);
        let store = RuntimeStatusStore::new(tx);

        let snap = snapshot_with_profile();
        // service_key would be "telegram:chat_42:work:claude" (4 segments)
        assert_eq!(snap.service_key(), "telegram:chat_42:work:claude");
        // route_key should be "telegram:chat_42" (2 segments)
        assert_eq!(snap.route.as_key(), "telegram:chat_42");

        store.upsert(snap);
        let agents = store.snapshot_agents();
        assert_eq!(agents.len(), 1);

        // The published id must be the route_key so RouteKey::from_key can parse it.
        let id = &agents[0].id;
        assert_eq!(id, "telegram:chat_42");
        assert!(
            RouteKey::from_key(id).is_some(),
            "published id must be parseable as RouteKey"
        );
    }
}
