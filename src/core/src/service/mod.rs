//! Service status manager: lightweight status registry for Dashboard display.
//!
//! This is a pure "status board" — it does NOT manage service lifecycles.
//! Data is synced in by `ServerDaemon` via hub events.
//!
//! Sub-registries:
//! - `channels`: IM channel plugins (keyed by channel kind, e.g. "feishu")
//! - `agents`: agent processes (keyed by hub agent key, e.g.
//!   "feishu:oc_001:default:claude")
//! - `tunnel`: tunnel process (at most one entry)
//! - `pty`: PTY sessions (reuses existing `SessionContext`)
//!
//! ## Module layout
//!
//! - [`status`]   — `ServiceStatus`, `ServiceMeta`, `spawn_tracked`
//! - [`entries`]  — per-kind entry structs (`ChannelEntry`, `TunnelEntry`, …)
//! - [`snapshot`] — API-facing snapshot types + `status_string`

mod entries;
mod snapshot;
mod status;

use std::sync::{Arc, Weak};

use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

use crate::channel_manager::monitor::{ChannelMonitor, ChannelRunStatus};

// parking_lot locks used throughout this module are fast, uncontended, and
// cover very short critical sections. They are _blocking_ locks, so the
// invariant across every call site below is: NEVER hold a guard across an
// `.await` point. If a lock needs to be held longer, convert that specific
// site to `tokio::sync::RwLock` — do not yield while holding parking_lot.

use crate::acp_hub::ACPHub;
use crate::pty::{unix_now_secs, Registry, SessionId};
use crate::tunnels::{TunnelManager, TunnelProvider};

pub use entries::{AgentStatusEntry, ChannelEntry};
pub use snapshot::{ApiServiceStatus, ServerMeta, ServiceInfo, StatusSnapshot};
pub use status::{spawn_tracked, ServiceMeta, ServiceStatus};

use snapshot::capitalize;

// ---------------------------------------------------------------------------
// ServiceStatusManager
// ---------------------------------------------------------------------------

/// Lightweight status registry for all running services.
/// Data is synced by `ServerDaemon` via hub events.
pub struct ServiceStatusManager {
    /// `ACPHub` back-ref (Weak). When present, agent snapshots are
    /// built by iterating `acp_hub.list()` and reading each pod's
    /// `state()` directly.
    acp_hub: RwLock<Weak<ACPHub>>,
    /// Channel plugin status (keyed by channel kind). Legacy store — the
    /// authoritative source once the monitor is installed is `channel_monitor`.
    /// Kept as a no-op compat layer for the tiny window before the monitor is
    /// registered.
    channels: DashMap<String, ChannelEntry>,
    /// `ChannelMonitor` back-ref (Weak to avoid cycle with `ChannelManager`).
    /// Set once at daemon boot via `set_channel_monitor`. When present,
    /// `snapshot()` and `kill_service("channels", ...)` route through it.
    channel_monitor: RwLock<Weak<ChannelMonitor>>,
    /// Tunnel registry (at most one per provider in normal operation).
    /// Owned directly — same lifecycle as `Services` — so the Dashboard
    /// snapshot code can read from it without a Weak-upgrade dance.
    tunnels: Arc<TunnelManager>,
    /// PTY sessions (reuses existing `Registry`).
    pub pty: Registry,
    /// Web server metadata.
    pub server_meta: ServerMeta,
    /// Convenience: the port the web server listens on.
    pub port: u16,
    /// Broadcast channel for real-time service status changes.
    change_tx: broadcast::Sender<()>,
}

impl ServiceStatusManager {
    pub fn new(port: u16) -> Self {
        // Capacity for the service status change broadcast. Slow /ws/services
        // subscribers that lag behind 64 events will receive a Lagged error
        // and re-sync on next receive. 64 is generous for status updates.
        let (change_tx, _) = broadcast::channel(64);
        Self {
            acp_hub: RwLock::new(Weak::new()),
            channels: DashMap::new(),
            channel_monitor: RwLock::new(Weak::new()),
            tunnels: TunnelManager::new(),
            pty: Arc::new(DashMap::new()),
            server_meta: ServerMeta {
                started_at: unix_now_secs(),
                port,
            },
            port,
            change_tx,
        }
    }

    // -----------------------------------------------------------------------
    // Channel monitor (set once at daemon boot)
    // -----------------------------------------------------------------------

    pub fn set_channel_monitor(&self, monitor: Weak<ChannelMonitor>) {
        *self.channel_monitor.write() = monitor;
    }

    pub fn channel_monitor(&self) -> Option<Arc<ChannelMonitor>> {
        self.channel_monitor.read().upgrade()
    }

    /// Clear all service entries. Called on daemon stop to prevent stale
    /// entries from persisting across restarts.
    pub fn clear(&self) {
        self.channels.clear();
        self.tunnels.clear();
        self.pty.clear();
        *self.acp_hub.write() = Weak::new();
        self.notify_change();
    }

    /// Shared `TunnelManager` — callers that need to subscribe to tunnel
    /// changes or iterate directly should use this, not the `Services`
    /// facade methods below. The facade methods are kept as thin
    /// delegates for backward-compat during the Phase 1g transition.
    pub fn tunnels(&self) -> Arc<TunnelManager> {
        Arc::clone(&self.tunnels)
    }

    // -----------------------------------------------------------------------
    // Change notification
    // -----------------------------------------------------------------------

    pub fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }

    pub fn notify_change(&self) {
        let _ = self.change_tx.send(());
    }

    /// Expose the change broadcast sender so `RuntimeStatusStore` can share it.
    pub fn change_tx(&self) -> broadcast::Sender<()> {
        self.change_tx.clone()
    }

    // -----------------------------------------------------------------------
    // ACPHub back-ref (set once at daemon boot). Agents snapshot is read
    // live from each pod via `acp_hub.list()` + `pod.state().await`.
    // -----------------------------------------------------------------------

    pub fn set_acp_hub(&self, hub: Weak<ACPHub>) {
        *self.acp_hub.write() = hub;
    }

    fn acp_hub(&self) -> Option<Arc<ACPHub>> {
        self.acp_hub.read().upgrade()
    }

    // -----------------------------------------------------------------------
    // Channels (registered by ServerDaemon after plugin start)
    // -----------------------------------------------------------------------

    pub fn register_channel(&self, kind: &str, abort_handle: AbortHandle) {
        let entry = ChannelEntry {
            meta: ServiceMeta::new(Some(abort_handle)),
        };
        self.channels.insert(kind.to_string(), entry);
        eprintln!("[ServiceStatus] registered channel: {}", kind);
        self.notify_change();
    }

    // -----------------------------------------------------------------------
    // Tunnel
    // -----------------------------------------------------------------------

    pub fn register_tunnel(&self, provider: TunnelProvider, abort_handle: AbortHandle) {
        self.tunnels.register(provider, abort_handle);
    }

    pub fn set_tunnel_url(&self, provider_key: &str, url: &str) {
        self.tunnels.set_url(provider_key, url);
    }

    pub fn has_tunnel_url(&self) -> bool {
        self.tunnels.has_url()
    }

    pub fn get_tunnel_url(&self) -> Option<String> {
        self.tunnels.first_url()
    }

    // -----------------------------------------------------------------------
    // Kill
    // -----------------------------------------------------------------------

    pub fn kill_service(&self, category: &str, key: &str) -> bool {
        match category {
            "channels" => {
                // Prefer the monitor: it distinguishes user-initiated stops
                // from involuntary crashes. Spawn an async task because
                // force_stop is async (needs to await runtime.shutdown).
                if let Some(monitor) = self.channel_monitor() {
                    let key = key.to_string();
                    tokio::spawn(async move {
                        if let Err(e) = monitor.force_stop(&key).await {
                            eprintln!("[ServiceStatus] force_stop({}) failed: {}", key, e);
                        }
                    });
                    self.notify_change();
                    return true;
                }
                if let Some(entry) = self.channels.get(key) {
                    entry.meta.kill();
                    self.notify_change();
                    return true;
                }
            }
            "tunnels" => {
                if self.tunnels.kill(key) {
                    self.notify_change();
                    return true;
                }
            }
            "pty" => {
                if let Ok(uuid) = uuid::Uuid::parse_str(key) {
                    self.pty.remove(&SessionId(uuid));
                    self.notify_change();
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    // -----------------------------------------------------------------------
    // Snapshot (for Dashboard API / WebSocket)
    // -----------------------------------------------------------------------

    pub async fn snapshot(&self) -> StatusSnapshot {
        use crate::state::StateSource;
        let pty_count = self.pty.len();
        let agents = self.agent_snapshot().await;
        let tunnels = self
            .tunnels
            .list()
            .await
            .into_iter()
            .map(|t| {
                let key = t.provider.as_str().to_string();
                let mut extra = serde_json::Map::new();
                extra.insert("provider".into(), t.provider.as_str().into());
                if let Some(ref url) = t.url {
                    extra.insert("url".into(), url.clone().into());
                }
                ServiceInfo {
                    id: key,
                    name: format!("Tunnel ({})", t.provider.as_str()),
                    status: (&t.status).into(),
                    uptime_secs: t.uptime_secs,
                    extra,
                }
            })
            .collect();

        StatusSnapshot {
            server: self.server_meta.clone(),
            tunnels,
            agents,
            channels: self.channel_snapshot(),
            pty_session_count: pty_count,
        }
    }

    /// Build the per-agent `ServiceInfo` list by iterating the `ACPHub`'s
    /// pods and reading each pod's live state. Replaces the previous
    /// `RuntimeStatusStore` projection cache (deleted).
    async fn agent_snapshot(&self) -> Vec<ServiceInfo> {
        let Some(hub) = self.acp_hub() else {
            return Vec::new();
        };
        let pods = hub.list();
        let now = unix_now_secs();
        let mut out = Vec::with_capacity(pods.len());
        for pod in pods {
            let st = pod.state().await;
            let route_key = pod.route.as_key();
            let service_key = format!(
                "{}:{}:{}:{}",
                pod.route.channel_kind,
                pod.route.chat_id,
                st.profile.clone().unwrap_or_else(|| "default".to_string()),
                st.cli_kind.clone().unwrap_or_else(|| "unknown".to_string()),
            );

            let mut extra = serde_json::Map::new();
            extra.insert("routeKey".into(), route_key.clone().into());
            extra.insert("channelKind".into(), pod.route.channel_kind.clone().into());
            extra.insert("chatId".into(), pod.route.chat_id.clone().into());
            if let Some(kind) = &st.cli_kind {
                extra.insert("kind".into(), kind.clone().into());
            }
            if let Some(profile) = &st.profile {
                extra.insert("profile".into(), profile.clone().into());
            }
            if let Some(sid) = &st.session_id {
                extra.insert("sessionId".into(), sid.clone().into());
            }
            extra.insert("busy".into(), st.busy.into());
            if let Some(err) = &st.failed {
                extra.insert("error".into(), err.clone().into());
            }
            if let Some(initialize) = &st.initialize {
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

            out.push(ServiceInfo {
                id: route_key,
                name: format!(
                    "{} ({})",
                    st.cli_kind.clone().unwrap_or_else(|| "agent".to_string()),
                    service_key,
                ),
                status: match &st.failed {
                    Some(error) => ApiServiceStatus::Failed { error: error.clone() },
                    None => ApiServiceStatus::Running,
                },
                uptime_secs: now.saturating_sub(pod.started_at()),
                extra,
            });
        }
        out
    }

    /// Build the per-channel `ServiceInfo` list. Prefers the `ChannelMonitor`
    /// when registered (rich status: running / spawning / crashed / stopped
    /// with reason + crash_count + last_seen_age + restart_in_secs). Falls
    /// back to the legacy `channels` `DashMap` for the narrow window before
    /// the monitor is installed.
    fn channel_snapshot(&self) -> Vec<ServiceInfo> {
        if let Some(monitor) = self.channel_monitor() {
            return monitor
                .snapshot()
                .into_iter()
                .map(|s| {
                    let mut extra = serde_json::Map::new();
                    if !s.reason.is_empty() {
                        extra.insert("reason".into(), s.reason.clone().into());
                    }
                    extra.insert(
                        "crash_count".into(),
                        serde_json::Value::from(s.crash_count),
                    );
                    extra.insert(
                        "last_seen_age_secs".into(),
                        serde_json::Value::from(s.last_seen_age_secs),
                    );
                    extra.insert(
                        "restart_in_secs".into(),
                        serde_json::Value::from(s.restart_in_secs),
                    );
                    let reason_opt = if s.reason.is_empty() {
                        None
                    } else {
                        Some(s.reason.clone())
                    };
                    let status = match s.status {
                        ChannelRunStatus::Running => ApiServiceStatus::Running,
                        ChannelRunStatus::NotStarted => ApiServiceStatus::NotStarted,
                        ChannelRunStatus::Spawning => ApiServiceStatus::Spawning,
                        ChannelRunStatus::Stopped => {
                            ApiServiceStatus::Stopped { reason: reason_opt }
                        }
                        ChannelRunStatus::Crashed => ApiServiceStatus::Crashed,
                    };
                    ServiceInfo {
                        id: s.kind.clone(),
                        name: capitalize(&s.kind),
                        status,
                        uptime_secs: s.last_seen_age_secs, // best-effort
                        extra,
                    }
                })
                .collect();
        }
        self.channels
            .iter()
            .map(|entry| {
                let key = entry.key().clone();
                ServiceInfo {
                    id: key.clone(),
                    name: capitalize(&key),
                    status: (&entry.meta.current_status()).into(),
                    uptime_secs: entry.meta.uptime_secs(),
                    extra: serde_json::Map::new(),
                }
            })
            .collect()
    }
}
