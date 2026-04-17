//! Channel plugin supervisor.
//!
//! Owns every channel plugin's lifecycle as central state + one global timer.
//! Runs as a single `tokio::task` loop that every 5 seconds scans the channel
//! registry and acts based on each channel's status:
//!
//! - `NotStarted` → spawn
//! - `Running`    → watchdog check (last heartbeat > 90s ⇒ kill + restart)
//! - `Crashed`    → if `restart_at` reached, respawn
//! - `Spawning`   → wait (spawn happens in a background task)
//! - `Stopped`    → no action (user-initiated)
//!
//! Failure paths:
//! 1. Plugin process crashed / externally killed / stdin closed → bridge task
//!    calls `mark_crashed()` when `handle_io.await` returns.
//! 2. Plugin event loop frozen → 90s watchdog (this module's `tick`).
//! 3. Plugin alive but IM API disconnected → plugin's own `healthCheck()` in
//!    the SDK gates the `_va/heartbeat` emit, falls through to path (2).
//!
//! Distinguishing intent:
//! - External kill / crash → intent = `None` → Crashed with 15s backoff
//! - Dashboard Kill        → intent = `Stop`    → Stopped, no respawn
//! - Dashboard Restart     → intent = `Restart` → Crashed(restart_at = now)
//! - Dashboard Start (from Stopped) → Crashed(restart_at = now)
//!
//! The existing OS-process invariants (guardian_task, ChildRegistry,
//! kill_on_drop) are **unchanged** — they still guarantee SIGKILL on daemon
//! stop. This module adds a separate liveness/respawn layer on top.

use std::sync::{Arc, Weak};
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;

use crate::acp_hub::ACPHub;
use crate::pty::unix_now_secs;

use super::manifest::ChannelPluginManifest;
use super::plugin_host::PluginHost;
use super::transport_stdio::StdioPluginRuntime;
use super::ChannelInput;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Timer tick interval. All status transitions happen on tick boundaries.
const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// If we haven't seen a heartbeat from a plugin in this many seconds, assume
/// it is stuck → kill + respawn.
const HEARTBEAT_TIMEOUT_SECS: u64 = 90;

/// Fixed wait between restart attempts. No exponential, no stability reset.
const RESTART_BACKOFF_SECS: u64 = 15;

// ---------------------------------------------------------------------------
// State enums (AtomicU8-friendly)
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
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::NotStarted,
            1 => Self::Spawning,
            2 => Self::Running,
            3 => Self::Crashed,
            _ => Self::Stopped,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Spawning => "spawning",
            Self::Running => "running",
            Self::Crashed => "crashed",
            Self::Stopped => "stopped",
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransitionIntent {
    None = 0,
    Stop = 1,
    Restart = 2,
}

impl TransitionIntent {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Stop,
            2 => Self::Restart,
            _ => Self::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-channel state
// ---------------------------------------------------------------------------

pub struct ChannelState {
    pub kind: String,
    pub manifest: ChannelPluginManifest,

    /// `ChannelRunStatus` stored as atomic for lock-free reads in tick/snapshot.
    status: AtomicU8,

    /// `TransitionIntent` — consumed by `mark_crashed` to distinguish user
    /// actions from actual crashes.
    intent: AtomicU8,

    /// Human-readable last transition reason ("no heartbeat", "io terminated
    /// (…)", "spawn failed: …"). Displayed on Dashboard.
    reason: RwLock<String>,

    /// Unix secs of last `_va/heartbeat` receipt. Bumped on register/spawn to
    /// give a grace window before first real heartbeat.
    last_seen_ts: AtomicU64,

    /// Unix secs of last crash. Purely informational for Dashboard.
    last_crash_ts: AtomicU64,

    /// How many times this channel has crashed since daemon start.
    crash_count: AtomicU32,

    /// Unix secs at which a respawn should be attempted. `0` means "not
    /// scheduled" (e.g. already running, or user-stopped).
    restart_at: AtomicU64,

    /// Arc to the currently-active `StdioPluginRuntime`. Swapped on respawn.
    /// `None` means no runtime is live (Crashed / Stopped / between states).
    current_runtime: Mutex<Option<Arc<StdioPluginRuntime>>>,
}

impl ChannelState {
    fn new(manifest: ChannelPluginManifest) -> Self {
        Self {
            kind: manifest.channel_kind.clone(),
            manifest,
            status: AtomicU8::new(ChannelRunStatus::NotStarted as u8),
            intent: AtomicU8::new(TransitionIntent::None as u8),
            reason: RwLock::new(String::new()),
            last_seen_ts: AtomicU64::new(unix_now_secs()),
            last_crash_ts: AtomicU64::new(0),
            crash_count: AtomicU32::new(0),
            restart_at: AtomicU64::new(0),
            current_runtime: Mutex::new(None),
        }
    }

    pub fn run_status(&self) -> ChannelRunStatus {
        ChannelRunStatus::from_u8(self.status.load(Ordering::Acquire))
    }

    fn set_status(&self, status: ChannelRunStatus) {
        self.status.store(status as u8, Ordering::Release);
    }

    fn set_reason(&self, reason: impl Into<String>) {
        *self.reason.write() = reason.into();
    }

    pub fn reason_snapshot(&self) -> String {
        self.reason.read().clone()
    }
}

// ---------------------------------------------------------------------------
// Snapshot (for Dashboard API)
// ---------------------------------------------------------------------------

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
// ChannelMonitor
// ---------------------------------------------------------------------------

pub struct ChannelMonitor {
    channels: DashMap<String, Arc<ChannelState>>,
    acp_hub: Arc<ACPHub>,
    input_tx: mpsc::UnboundedSender<ChannelInput>,

    /// Strong ref to PluginHost. `PluginHost` holds a `Weak<ChannelMonitor>`
    /// for the back-pointer used by bridge threads to call `mark_crashed`
    /// and `touch` — this avoids a reference cycle.
    plugin_host: Arc<PluginHost>,

    /// Broadcasts `()` when any channel status changes; subscribers re-query
    /// snapshot. Shared with `ServiceStatusManager::change_tx` so Dashboard
    /// gets a single unified stream.
    change_tx: tokio::sync::broadcast::Sender<()>,
}

impl ChannelMonitor {
    pub fn new(
        acp_hub: Arc<ACPHub>,
        input_tx: mpsc::UnboundedSender<ChannelInput>,
        plugin_host: Arc<PluginHost>,
        change_tx: tokio::sync::broadcast::Sender<()>,
    ) -> Arc<Self> {
        Arc::new(Self {
            channels: DashMap::new(),
            acp_hub,
            input_tx,
            plugin_host,
            change_tx,
        })
    }

    /// Register a channel manifest. Initial status is `NotStarted`; the next
    /// tick (within 5s) will transition to `Spawning`. For faster boot, a
    /// one-shot spawn task is kicked immediately.
    pub fn register(self: &Arc<Self>, manifest: ChannelPluginManifest) {
        let kind = manifest.channel_kind.clone();
        let state = Arc::new(ChannelState::new(manifest));
        self.channels.insert(kind.clone(), Arc::clone(&state));
        self.notify_change();

        // Immediate spawn instead of waiting for the 5s tick.
        let monitor = Arc::clone(self);
        tokio::spawn(async move {
            monitor.begin_spawn(state).await;
        });
    }

    /// Bump `last_seen_ts` — called on every `_va/heartbeat` arrival.
    pub fn touch(&self, kind: &str) {
        if let Some(entry) = self.channels.get(kind) {
            entry.last_seen_ts.store(unix_now_secs(), Ordering::Relaxed);
        }
    }

    /// Signal from the bridge thread that the plugin subprocess is dead.
    /// Also used internally by the watchdog path (intent-aware).
    pub fn mark_crashed(self: &Arc<Self>, kind: &str, reason: &str) {
        let Some(entry) = self.channels.get(kind) else {
            return;
        };
        let state = Arc::clone(&entry);
        drop(entry);

        // Consume intent atomically so two racing callers don't both observe Stop.
        let intent = TransitionIntent::from_u8(
            state.intent.swap(TransitionIntent::None as u8, Ordering::AcqRel),
        );

        state.current_runtime.lock().take();
        state.set_reason(reason);
        state.last_crash_ts.store(unix_now_secs(), Ordering::Relaxed);

        match intent {
            TransitionIntent::Stop => {
                state.set_status(ChannelRunStatus::Stopped);
                state.restart_at.store(0, Ordering::Relaxed);
                eprintln!(
                    "[monitor] {} → Stopped (user stop): {}",
                    state.kind, reason
                );
            }
            TransitionIntent::Restart => {
                state.set_status(ChannelRunStatus::Crashed);
                state
                    .restart_at
                    .store(unix_now_secs(), Ordering::Relaxed);
                eprintln!(
                    "[monitor] {} → Crashed (user restart, respawning immediately): {}",
                    state.kind, reason
                );
            }
            TransitionIntent::None => {
                state.set_status(ChannelRunStatus::Crashed);
                state.crash_count.fetch_add(1, Ordering::Relaxed);
                state
                    .restart_at
                    .store(unix_now_secs() + RESTART_BACKOFF_SECS, Ordering::Relaxed);
                eprintln!(
                    "[monitor] {} → Crashed (auto-respawn in {}s): {}",
                    state.kind, RESTART_BACKOFF_SECS, reason
                );
            }
        }
        self.notify_change();
    }

    // -----------------------------------------------------------------------
    // Force actions (Dashboard)
    // -----------------------------------------------------------------------

    pub async fn force_stop(self: &Arc<Self>, kind: &str) -> Result<(), String> {
        let Some(state) = self.channels.get(kind).map(|e| Arc::clone(&e)) else {
            return Err(format!("unknown channel: {}", kind));
        };
        state
            .intent
            .store(TransitionIntent::Stop as u8, Ordering::Release);
        let rt = state.current_runtime.lock().take();
        if let Some(rt) = rt {
            rt.shutdown().await;
            // Bridge exit will call mark_crashed, which consumes intent = Stop → Stopped.
        } else {
            // No live runtime — synthesize the mark_crashed outcome directly.
            self.mark_crashed(kind, "stopped by user");
        }
        Ok(())
    }

    pub async fn force_restart(self: &Arc<Self>, kind: &str) -> Result<(), String> {
        let Some(state) = self.channels.get(kind).map(|e| Arc::clone(&e)) else {
            return Err(format!("unknown channel: {}", kind));
        };
        state
            .intent
            .store(TransitionIntent::Restart as u8, Ordering::Release);
        let rt = state.current_runtime.lock().take();
        if let Some(rt) = rt {
            rt.shutdown().await;
            // Bridge exit → mark_crashed → Crashed(restart_at=now) → next tick spawns.
        } else {
            // Already dead — skip the bridge dance, jump straight to "ready to spawn".
            self.mark_crashed(kind, "restarted by user");
        }
        Ok(())
    }

    pub fn force_start(self: &Arc<Self>, kind: &str) -> Result<(), String> {
        let Some(state) = self.channels.get(kind).map(|e| Arc::clone(&e)) else {
            return Err(format!("unknown channel: {}", kind));
        };
        // Only meaningful from Stopped / Crashed. For Running / Spawning we
        // just no-op — status is already on a live path.
        match state.run_status() {
            ChannelRunStatus::Stopped | ChannelRunStatus::Crashed | ChannelRunStatus::NotStarted => {
                state.set_status(ChannelRunStatus::Crashed);
                state.set_reason("started by user");
                state
                    .restart_at
                    .store(unix_now_secs(), Ordering::Relaxed);
                self.notify_change();
            }
            _ => {}
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Timer loop
    // -----------------------------------------------------------------------

    /// Scan every channel and act based on its status. Called by
    /// `run_monitor_loop` every `TICK_INTERVAL`.
    pub async fn tick(self: &Arc<Self>) {
        let now = unix_now_secs();
        let mut to_spawn: Vec<Arc<ChannelState>> = Vec::new();

        for entry in self.channels.iter() {
            let state = Arc::clone(&entry);
            match state.run_status() {
                ChannelRunStatus::NotStarted => to_spawn.push(state),
                ChannelRunStatus::Running => {
                    let last = state.last_seen_ts.load(Ordering::Relaxed);
                    if now.saturating_sub(last) > HEARTBEAT_TIMEOUT_SECS {
                        eprintln!(
                            "[monitor] {} watchdog fired (last_seen {}s ago > {}s)",
                            state.kind,
                            now.saturating_sub(last),
                            HEARTBEAT_TIMEOUT_SECS
                        );
                        // Kill the runtime; its bridge will exit and call
                        // mark_crashed (intent=None → Crashed + backoff).
                        let rt = state.current_runtime.lock().take();
                        if let Some(rt) = rt {
                            rt.shutdown().await;
                        } else {
                            // No live runtime despite Running status — recover by
                            // marking crashed directly.
                            self.mark_crashed(&state.kind, "no heartbeat (no runtime)");
                        }
                    }
                }
                ChannelRunStatus::Crashed => {
                    let at = state.restart_at.load(Ordering::Relaxed);
                    if at != 0 && now >= at {
                        to_spawn.push(state);
                    }
                }
                ChannelRunStatus::Spawning | ChannelRunStatus::Stopped => {
                    // nothing
                }
            }
        }

        // Fire spawns outside the iteration. Each spawn runs in its own task
        // so a slow-spawning plugin never blocks the timer loop for others.
        for state in to_spawn {
            let monitor = Arc::clone(self);
            tokio::spawn(async move {
                monitor.begin_spawn(state).await;
            });
        }
    }

    async fn begin_spawn(self: &Arc<Self>, state: Arc<ChannelState>) {
        // Guard: if we raced with another tick or a force_* action, bail out
        // so we don't double-spawn.
        let prev = state
            .status
            .swap(ChannelRunStatus::Spawning as u8, Ordering::AcqRel);
        let prev = ChannelRunStatus::from_u8(prev);
        if matches!(prev, ChannelRunStatus::Spawning) {
            return;
        }
        state.restart_at.store(0, Ordering::Relaxed);
        state.set_reason("spawning");
        self.notify_change();

        let spawn_result = StdioPluginRuntime::spawn(
            state.manifest.clone(),
            self.input_tx.clone(),
            Arc::clone(&self.acp_hub),
            Arc::clone(&self.plugin_host),
        )
        .await;

        match spawn_result {
            Ok(runtime) => {
                let runtime = Arc::new(runtime);
                *state.current_runtime.lock() = Some(Arc::clone(&runtime));
                // Publish into plugin_host.runtimes so send_output can route
                // to the fresh runtime.
                self.plugin_host
                    .replace_stdio_runtime(&state.kind, runtime)
                    .await;
                state.set_status(ChannelRunStatus::Running);
                state.set_reason("");
                // Grace window before first real heartbeat counts.
                state
                    .last_seen_ts
                    .store(unix_now_secs(), Ordering::Relaxed);
                eprintln!("[monitor] {} → Running", state.kind);
                self.notify_change();
            }
            Err(e) => {
                state.set_status(ChannelRunStatus::Crashed);
                state.set_reason(format!("spawn failed: {}", e));
                state
                    .restart_at
                    .store(unix_now_secs() + RESTART_BACKOFF_SECS, Ordering::Relaxed);
                state.crash_count.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "[monitor] {} → Crashed (spawn error, retry in {}s): {}",
                    state.kind, RESTART_BACKOFF_SECS, e
                );
                self.notify_change();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Snapshot
    // -----------------------------------------------------------------------

    pub fn snapshot(&self) -> Vec<ChannelStatusSnapshot> {
        let now = unix_now_secs();
        let mut out: Vec<ChannelStatusSnapshot> = self
            .channels
            .iter()
            .map(|entry| {
                let state = entry.value();
                let last_seen = state.last_seen_ts.load(Ordering::Relaxed);
                let restart_at = state.restart_at.load(Ordering::Relaxed);
                ChannelStatusSnapshot {
                    kind: state.kind.clone(),
                    status: state.run_status(),
                    reason: state.reason_snapshot(),
                    crash_count: state.crash_count.load(Ordering::Relaxed),
                    last_seen_age_secs: now.saturating_sub(last_seen),
                    restart_in_secs: restart_at.saturating_sub(now),
                    started_at: last_seen,
                }
            })
            .collect();
        out.sort_by(|a, b| a.kind.cmp(&b.kind));
        out
    }

    fn notify_change(&self) {
        let _ = self.change_tx.send(());
    }

    /// Expose the broadcast sender so callers (e.g. `ServiceStatusManager`)
    /// can unify change notifications into a single subscribe stream.
    pub fn change_tx_clone(&self) -> tokio::sync::broadcast::Sender<()> {
        self.change_tx.clone()
    }

    /// Subscribe to change notifications.
    pub fn subscribe_changes(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }
}

// ---------------------------------------------------------------------------
// Public API from PluginHost weak-ref
// ---------------------------------------------------------------------------

/// Convenience for bridge threads which hold a `Weak<ChannelMonitor>` via
/// `PluginHost::monitor_weak()`. No-op if the monitor has already been dropped.
pub fn mark_crashed_weak(weak: &Weak<ChannelMonitor>, kind: &str, reason: &str) {
    if let Some(monitor) = weak.upgrade() {
        monitor.mark_crashed(kind, reason);
    }
}

pub fn touch_weak(weak: &Weak<ChannelMonitor>, kind: &str) {
    if let Some(monitor) = weak.upgrade() {
        monitor.touch(kind);
    }
}

// ---------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------

/// Drive the monitor's tick. Spawn this as a top-level tokio task at daemon
/// boot. The `shutdown_rx` is signaled on clean daemon stop.
pub async fn run_monitor_loop(
    monitor: Arc<ChannelMonitor>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    // Skip the immediate tick so we don't race with `register`'s immediate spawn.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // consume the first immediate tick

    eprintln!("[monitor] run_monitor_loop started (tick every {}s)", TICK_INTERVAL.as_secs());
    loop {
        tokio::select! {
            _ = ticker.tick() => monitor.tick().await,
            _ = shutdown_rx.recv() => {
                eprintln!("[monitor] run_monitor_loop shutting down");
                break;
            }
        }
    }
}
