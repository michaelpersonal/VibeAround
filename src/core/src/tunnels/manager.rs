//! `TunnelManager` — owns the registry of active tunnels.
//!
//! Extracted from `ServiceStatusManager` so tunnels follow the same
//! "per-domain kernel manager + `StateSource` trait" pattern as
//! `ChannelMonitor` and `ACPHub`. Consumers read tunnel state via
//! `list()` / `subscribe_changes()` and no longer go through the
//! `Services` facade (which will be dismantled in a later commit).

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

use crate::service::ServiceStatus;
use crate::service::ServiceMeta;

use super::TunnelProvider;

/// One registered tunnel (at most one per provider in normal operation).
/// Held internally by `TunnelManager`; external consumers see the
/// value-typed [`TunnelStatus`] via [`TunnelManager::list`].
pub struct TunnelEntry {
    pub meta: ServiceMeta,
    pub provider: TunnelProvider,
    /// Public URL once the backend has finished connecting. `None` while
    /// the backend is still starting up.
    pub url: Option<String>,
}

/// Value-typed view of a single tunnel's current state, suitable for
/// handing out to consumers that iterate the registry. This is what
/// `StateSource::list` returns.
///
/// We don't hand out the raw `TunnelEntry` because its `ServiceMeta`
/// holds a `Box<dyn Fn>` kill closure that isn't `Clone`, so a
/// snapshot type is the most honest interface.
#[derive(Debug, Clone)]
pub struct TunnelStatus {
    pub provider: TunnelProvider,
    pub url: Option<String>,
    pub status: ServiceStatus,
    pub uptime_secs: u64,
}

/// Owner of the live tunnel registry. Wire shells (HTTP, TUI, CLI) use
/// [`StateSource`] to inspect it; internal code that needs to register a
/// newly-spawned tunnel or update its URL calls the mutators directly.
///
/// [`StateSource`]: crate::state::StateSource
pub struct TunnelManager {
    tunnels: DashMap<String, TunnelEntry>,
    change_tx: broadcast::Sender<()>,
}

impl TunnelManager {
    pub fn new() -> Arc<Self> {
        let (change_tx, _) = broadcast::channel(32);
        Arc::new(Self {
            tunnels: DashMap::new(),
            change_tx,
        })
    }

    /// Register a freshly-spawned tunnel. The caller supplies the tokio
    /// abort handle of the background task that keeps the tunnel alive;
    /// a later `kill_via_key` uses it to stop the tunnel.
    pub fn register(&self, provider: TunnelProvider, abort_handle: AbortHandle) {
        self.tunnels.insert(
            provider.as_str().to_string(),
            TunnelEntry {
                meta: ServiceMeta::new(Some(abort_handle)),
                provider,
                url: None,
            },
        );
        self.notify_change();
    }

    /// Set the public URL once the backend reports it.
    pub fn set_url(&self, provider_key: &str, url: &str) {
        if let Some(mut entry) = self.tunnels.get_mut(provider_key) {
            entry.url = Some(url.to_string());
        }
        self.notify_change();
    }

    /// Kill the tunnel matching `provider_key` and remove it from the
    /// registry. Returns `true` if an entry was found and killed.
    pub fn kill(&self, provider_key: &str) -> bool {
        let killed = if let Some(entry) = self.tunnels.get(provider_key) {
            entry.meta.kill();
            true
        } else {
            false
        };
        if killed {
            self.tunnels.remove(provider_key);
            self.notify_change();
        }
        killed
    }

    /// Clear all tunnels. Called on daemon stop.
    pub fn clear(&self) {
        self.tunnels.clear();
        self.notify_change();
    }

    /// True if any registered tunnel has a URL (i.e. at least one tunnel
    /// is fully up).
    pub fn has_url(&self) -> bool {
        self.tunnels.iter().any(|entry| entry.url.is_some())
    }

    /// First public URL we can find, or `None` if no tunnel is up yet.
    pub fn first_url(&self) -> Option<String> {
        self.tunnels.iter().find_map(|entry| entry.url.clone())
    }

    fn notify_change(&self) {
        let _ = self.change_tx.send(());
    }
}

impl crate::state::StateSource for TunnelManager {
    type Entry = TunnelStatus;

    async fn list(&self) -> Vec<Self::Entry> {
        self.tunnels
            .iter()
            .map(|entry| TunnelStatus {
                provider: entry.provider,
                url: entry.url.clone(),
                status: entry.meta.current_status(),
                uptime_secs: entry.meta.uptime_secs(),
            })
            .collect()
    }

    fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }
}
