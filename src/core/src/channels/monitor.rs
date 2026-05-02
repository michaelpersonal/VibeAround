//! `ChannelMonitor` — thin facade over [`process::Supervisor`] that
//! presents the channel-plugin-specific public API consumed by the
//! Dashboard, the REST/WS routes, and the `StateSource` trait.
//!
//! All real lifecycle work (spawn, kill, restart, watchdog, status
//! broadcast) now lives in `process::Supervisor`. This module owns:
//!
//! - The mapping from the user-facing `channel_kind` string (`"feishu"`)
//!   to the opaque `ProcessId` returned by the supervisor.
//! - The conversion from the generic `ProcessSnapshot` into the
//!   Dashboard-facing `ChannelStatusSnapshot` (status string + age in
//!   seconds, etc.).
//! - A small forwarder task that fans the supervisor's typed
//!   `ProcessEvent` stream down to the `()` channel that
//!   [`StateSource::subscribe_changes`] exposes.
//!
//! Legacy call sites (`ChannelBridgeHandler`, `StdioPluginRuntime`) still
//! refer to `ChannelMonitor` by name — this file keeps their surface stable.
//!
//! [`StateSource::subscribe_changes`]: crate::state::StateSource::subscribe_changes

use std::sync::{Arc, Weak};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{broadcast, mpsc};

use crate::conversations::ConversationManager;
use crate::process::bridge::{BridgeFactory, ProcessBridge};
use crate::process::registry::ProcessKind;
use crate::process::supervisor::{ProcessEvent, ProcessId, RestartPolicy, SpawnSpec, Supervisor};

use super::manifest::ChannelPluginManifest;
use super::plugin_bridge::ChannelPluginBridge;
use super::plugin_host::PluginHost;
use super::transport_stdio::StdioPluginRuntime;
use super::ChannelInput;

// ---------------------------------------------------------------------------
// Tunables — kept at module scope so the Dashboard can display them.
// ---------------------------------------------------------------------------

pub const RESTART_BACKOFF: Duration = Duration::from_secs(15);
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(90);

// ---------------------------------------------------------------------------
// Public status types — shape preserved from pre-migration code so the
// REST handlers + api_types shim don't need to change.
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRunStatus {
    NotStarted = 0,
    Spawning = 1,
    Running = 2,
    Crashed = 3,
    Stopped = 4,
}

impl ChannelRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Spawning => "spawning",
            Self::Running => "running",
            Self::Crashed => "crashed",
            Self::Stopped => "stopped",
        }
    }

    fn from_process(status: crate::process::supervisor::ProcessStatus) -> Self {
        use crate::process::supervisor::ProcessStatus as P;
        match status {
            P::NotStarted => Self::NotStarted,
            P::Spawning => Self::Spawning,
            P::Running => Self::Running,
            P::Crashed => Self::Crashed,
            P::Stopped => Self::Stopped,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChannelStatusSnapshot {
    pub kind: String,
    pub status: ChannelRunStatus,
    pub reason: String,
    pub crash_count: u32,
    pub last_seen_age_secs: u64,
    pub restart_in_secs: u64,
    pub started_at: u64,
}

// ---------------------------------------------------------------------------
// Facade
// ---------------------------------------------------------------------------

pub struct ChannelMonitor {
    supervisor: Arc<Supervisor>,
    kinds: DashMap<String, ProcessId>,
    conversation_manager: Arc<ConversationManager>,
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    plugin_host: Arc<PluginHost>,
    /// Republished `()` stream for `StateSource::subscribe_changes`.
    change_tx: broadcast::Sender<()>,
}

impl ChannelMonitor {
    /// Build the monitor, spawn the supervisor's tick loop, and start a
    /// forwarder that maps supervisor `ProcessEvent`s (filtered to
    /// `ChannelPlugin`) into the `()` notifications that consumers
    /// subscribe to via [`StateSource::subscribe_changes`].
    ///
    /// [`StateSource::subscribe_changes`]: crate::state::StateSource::subscribe_changes
    pub fn new(
        conversation_manager: Arc<ConversationManager>,
        input_tx: mpsc::UnboundedSender<ChannelInput>,
        plugin_host: Arc<PluginHost>,
        change_tx: broadcast::Sender<()>,
    ) -> Arc<Self> {
        let supervisor = Supervisor::global();

        let forwarder_rx = supervisor.subscribe();
        let forwarder_tx = change_tx.clone();
        tokio::spawn(forward_events(forwarder_rx, forwarder_tx));

        Arc::new(Self {
            supervisor,
            kinds: DashMap::new(),
            conversation_manager,
            input_tx,
            plugin_host,
            change_tx,
        })
    }

    /// Register a channel plugin. The supervisor spawns immediately
    /// (no wait for the next tick) and keeps it alive under an
    /// `OnCrash` policy with a 15-second backoff and a 90-second
    /// heartbeat watchdog.
    pub fn register(self: &Arc<Self>, manifest: ChannelPluginManifest) {
        let kind = manifest.channel_kind.clone();

        let spec = SpawnSpec::new("node")
            .arg(manifest.entry_path.to_string_lossy().to_string())
            .cwd(manifest.plugin_dir.clone());

        let factory = build_bridge_factory(
            manifest,
            Arc::clone(&self.input_tx_owned()),
            Arc::clone(&self.conversation_manager),
            Arc::clone(&self.plugin_host),
        );

        let id = self.supervisor.register(
            ProcessKind::ChannelPlugin,
            kind.clone(),
            spec,
            RestartPolicy::OnCrash {
                backoff: RESTART_BACKOFF,
                watchdog: Some(HEARTBEAT_TIMEOUT),
            },
            factory,
        );
        self.kinds.insert(kind, id);
    }

    /// Bump the liveness timestamp — called on every `_va/heartbeat`
    /// from the plugin. No-op if the channel was deregistered mid-flight.
    pub fn touch(&self, kind: &str) {
        if let Some(id) = self.lookup(kind) {
            self.supervisor.touch(id);
        }
    }

    pub async fn force_stop(self: &Arc<Self>, kind: &str) -> Result<(), String> {
        let id = self
            .lookup(kind)
            .ok_or_else(|| format!("unknown channel: {}", kind))?;
        self.supervisor
            .force_stop(id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn force_restart(self: &Arc<Self>, kind: &str) -> Result<(), String> {
        let id = self
            .lookup(kind)
            .ok_or_else(|| format!("unknown channel: {}", kind))?;
        self.supervisor
            .force_restart(id)
            .await
            .map_err(|e| e.to_string())
    }

    pub fn force_start(self: &Arc<Self>, kind: &str) -> Result<(), String> {
        let id = self
            .lookup(kind)
            .ok_or_else(|| format!("unknown channel: {}", kind))?;
        self.supervisor.force_start(id).map_err(|e| e.to_string())
    }

    pub fn snapshot(&self) -> Vec<ChannelStatusSnapshot> {
        self.supervisor
            .snapshot()
            .into_iter()
            .filter(|p| p.kind == ProcessKind::ChannelPlugin)
            .map(|p| ChannelStatusSnapshot {
                kind: p.label,
                status: ChannelRunStatus::from_process(p.status),
                reason: p.reason,
                crash_count: p.crash_count,
                last_seen_age_secs: p.last_seen_age_secs,
                restart_in_secs: p.restart_in_secs,
                // `started_at` historically held the last_seen_ts. The
                // supervisor exposes the age instead, so back-compute an
                // absolute timestamp for the Dashboard.
                started_at: now_secs().saturating_sub(p.last_seen_age_secs),
            })
            .collect()
    }

    pub fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }

    /// Cooperative shutdown — cancels every live bridge and stops the
    /// supervisor tick loop. `ChildRegistry::kill_all()` is still the
    /// authoritative SIGKILL safety net on abrupt exits.
    pub async fn shutdown_all(&self) {
        self.supervisor.shutdown_all().await;
    }

    fn lookup(&self, kind: &str) -> Option<ProcessId> {
        self.kinds.get(kind).map(|entry| *entry.value())
    }

    /// Borrow the input sender as a per-call `Arc` for the factory. We
    /// wrap it so the factory closure stays `Fn` (not `FnOnce`).
    fn input_tx_owned(&self) -> Arc<mpsc::UnboundedSender<ChannelInput>> {
        Arc::new(self.input_tx.clone())
    }
}

// ---------------------------------------------------------------------------
// StateSource impl (unchanged contract)
// ---------------------------------------------------------------------------

impl crate::state::StateSource for ChannelMonitor {
    type Entry = ChannelStatusSnapshot;

    async fn list(&self) -> Vec<Self::Entry> {
        self.snapshot()
    }

    fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }
}

// ---------------------------------------------------------------------------
// Weak-ref touch helper — called from the plugin bridge's `_va/heartbeat`
// handler. `mark_crashed_weak` is gone: the supervisor observes the bridge's
// `BridgeExit` directly and no longer needs a weak back-pointer.
// ---------------------------------------------------------------------------

pub fn touch_weak(weak: &Weak<ChannelMonitor>, kind: &str) {
    if let Some(monitor) = weak.upgrade() {
        monitor.touch(kind);
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Forwarder that republishes supervisor events as `()` pings, filtered
/// to `ChannelPlugin` entries so the Dashboard WS doesn't re-render for
/// unrelated PTY / tunnel state.
async fn forward_events(mut rx: broadcast::Receiver<ProcessEvent>, tx: broadcast::Sender<()>) {
    loop {
        match rx.recv().await {
            Ok(event) if event.kind == ProcessKind::ChannelPlugin => {
                let _ = tx.send(());
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Build the per-respawn bridge factory. The factory is invoked once per
/// spawn attempt — it allocates a fresh output channel pair, registers a
/// new `StdioPluginRuntime` with the `PluginHost` so `send_output` routes
/// to the new bridge, and hands the bridge the receiving half.
fn build_bridge_factory(
    manifest: ChannelPluginManifest,
    input_tx: Arc<mpsc::UnboundedSender<ChannelInput>>,
    conversation_manager: Arc<ConversationManager>,
    plugin_host: Arc<PluginHost>,
) -> BridgeFactory {
    Box::new(move || {
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        plugin_host.replace_stdio_runtime(
            &manifest.channel_kind,
            Arc::new(StdioPluginRuntime::new(
                manifest.channel_kind.clone(),
                output_tx,
            )),
        );

        Box::new(ChannelPluginBridge {
            channel_kind: manifest.channel_kind.clone(),
            raw_config: manifest.raw_config.clone(),
            input_tx: (*input_tx).clone(),
            output_rx,
            conversation_manager: Arc::clone(&conversation_manager),
            plugin_host: Arc::clone(&plugin_host),
        }) as Box<dyn ProcessBridge>
    })
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
