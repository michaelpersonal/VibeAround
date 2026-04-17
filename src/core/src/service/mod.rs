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

use crate::pty::{unix_now_secs, Registry, SessionId};
use crate::runtime_status::RuntimeStatusStore;
use crate::tunnels::TunnelProvider;

pub use entries::{AgentStatusEntry, ChannelEntry, TunnelEntry};
pub use snapshot::{status_string, ServerMeta, ServiceInfo, StatusSnapshot};
pub use status::{spawn_tracked, ServiceMeta, ServiceStatus};

use snapshot::capitalize;

// ---------------------------------------------------------------------------
// ServiceStatusManager
// ---------------------------------------------------------------------------

/// Lightweight status registry for all running services.
/// Data is synced by `ServerDaemon` via hub events.
pub struct ServiceStatusManager {
    /// Runtime status store (event-driven, from `ACPHub` `HubEvent` stream).
    runtime_status: RwLock<Option<Arc<RuntimeStatusStore>>>,
    /// Channel plugin status (keyed by channel kind). Legacy store — the
    /// authoritative source once the monitor is installed is `channel_monitor`.
    /// Kept as a no-op compat layer for the tiny window before the monitor is
    /// registered.
    channels: DashMap<String, ChannelEntry>,
    /// `ChannelMonitor` back-ref (Weak to avoid cycle with `ChannelManager`).
    /// Set once at daemon boot via `set_channel_monitor`. When present,
    /// `snapshot()` and `kill_service("channels", ...)` route through it.
    channel_monitor: RwLock<Weak<ChannelMonitor>>,
    /// Tunnel status (at most one).
    tunnels: DashMap<String, TunnelEntry>,
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
            runtime_status: RwLock::new(None),
            channels: DashMap::new(),
            channel_monitor: RwLock::new(Weak::new()),
            tunnels: DashMap::new(),
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
        *self.runtime_status.write() = None;
        self.notify_change();
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
    // Runtime status (event-driven from ACPHub)
    // -----------------------------------------------------------------------

    pub fn set_runtime_status(&self, store: Arc<RuntimeStatusStore>) {
        *self.runtime_status.write() = Some(store);
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
        let entry = TunnelEntry {
            meta: ServiceMeta::new(Some(abort_handle)),
            provider,
            url: None,
        };
        self.tunnels.insert(provider.as_str().to_string(), entry);
        self.notify_change();
    }

    pub fn set_tunnel_url(&self, provider_key: &str, url: &str) {
        if let Some(mut entry) = self.tunnels.get_mut(provider_key) {
            entry.url = Some(url.to_string());
            self.notify_change();
        }
    }

    pub fn has_tunnel_url(&self) -> bool {
        self.tunnels.iter().any(|entry| entry.url.is_some())
    }

    pub fn get_tunnel_url(&self) -> Option<String> {
        self.tunnels.iter().find_map(|entry| entry.url.clone())
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
                if let Some(entry) = self.tunnels.get(key) {
                    entry.meta.kill();
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

    pub fn snapshot(&self) -> StatusSnapshot {
        let pty_count = self.pty.len();

        let agents = self
            .runtime_status
            .read()
            .as_ref()
            .map(|store| store.snapshot_agents())
            .unwrap_or_default();

        StatusSnapshot {
            server: self.server_meta.clone(),
            tunnels: self
                .tunnels
                .iter()
                .map(|entry| {
                    let key = entry.key().clone();
                    ServiceInfo {
                        id: key.clone(),
                        name: format!("Tunnel ({})", entry.provider.as_str()),
                        status: status_string(&entry.meta.current_status()),
                        uptime_secs: entry.meta.uptime_secs(),
                        extra: {
                            let mut m = serde_json::Map::new();
                            m.insert("provider".into(), entry.provider.as_str().into());
                            if let Some(ref url) = entry.url {
                                m.insert("url".into(), url.clone().into());
                            }
                            m
                        },
                    }
                })
                .collect(),
            agents,
            channels: self.channel_snapshot(),
            pty_session_count: pty_count,
        }
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
                    extra.insert("reason".into(), s.reason.into());
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
                    let status_str = match s.status {
                        ChannelRunStatus::Running => "running".to_string(),
                        ChannelRunStatus::NotStarted => "not_started".to_string(),
                        ChannelRunStatus::Spawning => "spawning".to_string(),
                        ChannelRunStatus::Stopped => "stopped".to_string(),
                        ChannelRunStatus::Crashed => "crashed".to_string(),
                    };
                    ServiceInfo {
                        id: s.kind.clone(),
                        name: capitalize(&s.kind),
                        status: status_str,
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
                    status: status_string(&entry.meta.current_status()),
                    uptime_secs: entry.meta.uptime_secs(),
                    extra: serde_json::Map::new(),
                }
            })
            .collect()
    }
}
