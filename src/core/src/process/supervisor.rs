//! `Supervisor` — owns the lifecycle of every supervised subprocess.
//!
//! This replaces the ad-hoc spawn/kill/restart paths that previously lived
//! inside `channels::monitor`, `agent::runtime`, and friends. Managers hand
//! the supervisor a `SpawnSpec` plus a `BridgeFactory` at `register()`
//! time, and from then on the supervisor:
//!
//! - Spawns the child process (via `process::env::command`, which injects
//!   the enriched login-shell env) and transfers the `Child` to the global
//!   [`ChildRegistry`].
//! - Invokes the factory on every (re)spawn to build a fresh
//!   [`ProcessBridge`], hands the bridge the stdio pipes, and runs it to
//!   completion in a task.
//! - Drives a state machine (`NotStarted` → `Spawning` → `Running` →
//!   `Crashed` → `Spawning` …) on a single 5-second tick loop that honors
//!   the [`RestartPolicy`] attached to each process.
//! - Broadcasts every status change on a `tokio::sync::broadcast` channel
//!   so dashboards, HTTP handlers, and other subscribers only subscribe
//!   once instead of polling per-module monitors.
//!
//! The supervisor does NOT know anything about the protocol spoken over
//! the stdio pipes — that is entirely the bridge's concern. It also does
//! NOT take responsibility for SIGKILLing children on abrupt daemon
//! shutdown — that is [`ChildRegistry::kill_all`]'s job. The supervisor
//! only drives the happy-path cancel + drop sequence.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::process::Command;
use tokio::sync::{broadcast, watch};

use crate::process::bridge::{BridgeExit, BridgeFactory, StdioPipes};
use crate::process::env;
use crate::process::error::{ProcessError, ProcessResult};
use crate::process::registry::{ChildRegistry, ProcessKind};
use crate::proc_log;

/// Tick interval for the supervisor's scan loop.
pub const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Unique id for a supervised process within one Supervisor instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcessId(pub u64);

impl std::fmt::Display for ProcessId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// SpawnSpec + RestartPolicy (caller-provided)
// ---------------------------------------------------------------------------

/// Recipe for spawning the child process. The supervisor uses this on every
/// (re)spawn — the bridge factory is invoked fresh each time.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub extra_env: Vec<(String, String)>,
    /// If `true`, the bridge receives `stderr` via [`StdioPipes`]. If
    /// `false` (default), the supervisor spawns a task that logs each line
    /// via [`tracing::info!`] with the process's kind+label fields.
    pub capture_stderr: bool,
}

impl SpawnSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            extra_env: Vec::new(),
            capture_stderr: false,
        }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, a: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(a.into_iter().map(|s| s.into()));
        self
    }

    pub fn cwd(mut self, p: impl Into<std::path::PathBuf>) -> Self {
        self.cwd = Some(p.into());
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.extra_env.push((k.into(), v.into()));
        self
    }

    pub fn capture_stderr(mut self, on: bool) -> Self {
        self.capture_stderr = on;
        self
    }
}

/// What to do when a supervised process dies.
#[derive(Debug, Clone, Copy)]
pub enum RestartPolicy {
    /// Move to `Stopped` on any exit. The owning manager decides whether
    /// to re-register. Used by `AcpAgent` and `Pty`.
    Never,
    /// On unintended exit (crash / protocol error), schedule a respawn
    /// after `backoff`. If `watchdog` is `Some`, the supervisor kills
    /// processes whose `touch()` timestamp is older than the watchdog
    /// window — this catches frozen plugins that aren't emitting
    /// heartbeats. Used by `ChannelPlugin`.
    OnCrash {
        backoff: Duration,
        watchdog: Option<Duration>,
    },
}

impl RestartPolicy {
    fn backoff(&self) -> Option<Duration> {
        match self {
            RestartPolicy::Never => None,
            RestartPolicy::OnCrash { backoff, .. } => Some(*backoff),
        }
    }

    fn watchdog(&self) -> Option<Duration> {
        match self {
            RestartPolicy::Never => None,
            RestartPolicy::OnCrash { watchdog, .. } => *watchdog,
        }
    }
}

// ---------------------------------------------------------------------------
// Status + events
// ---------------------------------------------------------------------------

/// Lifecycle status of a supervised process. Stored as an `AtomicU8`.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    NotStarted = 0,
    Spawning = 1,
    Running = 2,
    Crashed = 3,
    Stopped = 4,
}

impl ProcessStatus {
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

/// Distinguishes a user action from an actual crash so that force_stop and
/// force_restart survive the race with a bridge-thread `mark_exit`.
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

/// Public read-only snapshot of a supervised process — safe to expose via
/// HTTP / dashboard without leaking internal state.
#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub id: ProcessId,
    pub kind: ProcessKind,
    pub label: String,
    pub status: ProcessStatus,
    pub reason: String,
    pub crash_count: u32,
    pub last_seen_age_secs: u64,
    pub restart_in_secs: u64,
}

/// Broadcast payload for status changes. Subscribers receive which process
/// changed and re-read via [`Supervisor::snapshot`] if they need details —
/// matching the existing `StateSource` convention.
#[derive(Debug, Clone)]
pub struct ProcessEvent {
    pub id: ProcessId,
    pub kind: ProcessKind,
    pub status: ProcessStatus,
}

// ---------------------------------------------------------------------------
// Internal per-process state
// ---------------------------------------------------------------------------

struct SupervisedProcess {
    id: ProcessId,
    kind: ProcessKind,
    label: String,
    spec: SpawnSpec,
    policy: RestartPolicy,
    factory: BridgeFactory,

    status: AtomicU8,
    intent: AtomicU8,
    reason: RwLock<String>,

    last_seen_ts: AtomicU64,
    last_crash_ts: AtomicU64,
    crash_count: AtomicU32,
    restart_at: AtomicU64,

    /// Cancel signal for the currently-running bridge. `None` between runs.
    cancel_tx: RwLock<Option<watch::Sender<bool>>>,

    /// `ChildRegistry` id for the currently-running spawn. Cleared on
    /// exit so the `Child` gets removed from the registry and reaped by
    /// [`Supervisor::handle_bridge_exit`] — otherwise every respawn
    /// leaks an entry and, on Unix, an unreaped zombie.
    current_registry_id: parking_lot::Mutex<Option<u64>>,
}

impl SupervisedProcess {
    fn set_status(&self, s: ProcessStatus) {
        self.status.store(s as u8, Ordering::Release);
    }

    fn status(&self) -> ProcessStatus {
        ProcessStatus::from_u8(self.status.load(Ordering::Acquire))
    }

    fn set_reason(&self, r: impl Into<String>) {
        *self.reason.write() = r.into();
    }
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

pub struct Supervisor {
    registry: Arc<ChildRegistry>,
    processes: RwLock<HashMap<ProcessId, Arc<SupervisedProcess>>>,
    next_id: parking_lot::Mutex<u64>,
    change_tx: broadcast::Sender<ProcessEvent>,
    tick_loop_started: parking_lot::Mutex<bool>,
}

impl Supervisor {
    pub fn new(registry: Arc<ChildRegistry>) -> Arc<Self> {
        let (change_tx, _) = broadcast::channel(64);
        Arc::new(Self {
            registry,
            processes: RwLock::new(HashMap::new()),
            next_id: parking_lot::Mutex::new(1),
            change_tx,
            tick_loop_started: parking_lot::Mutex::new(false),
        })
    }

    /// Process-wide singleton. Bound to `ChildRegistry::global()`; the
    /// tick loop is auto-started on first access and runs for the
    /// remainder of the process lifetime — [`shutdown_all`] only drains
    /// the current process table so a subsequent daemon start gets a
    /// clean slate while the loop keeps ticking. Must be called from
    /// inside a tokio runtime.
    ///
    /// [`shutdown_all`]: Supervisor::shutdown_all
    pub fn global() -> Arc<Self> {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<Supervisor>> = OnceLock::new();
        Arc::clone(INSTANCE.get_or_init(|| {
            let sup = Supervisor::new(ChildRegistry::global());
            sup.spawn_tick_loop();
            sup
        }))
    }

    /// Register a new supervised process. Returns an opaque `ProcessId`
    /// that the caller uses for later `force_*`, `touch`, and status calls.
    /// The first spawn attempt is kicked off immediately (not waiting for
    /// the next tick).
    pub fn register(
        self: &Arc<Self>,
        kind: ProcessKind,
        label: impl Into<String>,
        spec: SpawnSpec,
        policy: RestartPolicy,
        factory: BridgeFactory,
    ) -> ProcessId {
        let label = label.into();
        let id = {
            let mut next = self.next_id.lock();
            let id = *next;
            *next = next.wrapping_add(1);
            ProcessId(id)
        };

        let proc = Arc::new(SupervisedProcess {
            id,
            kind,
            label: label.clone(),
            spec,
            policy,
            factory,
            status: AtomicU8::new(ProcessStatus::NotStarted as u8),
            intent: AtomicU8::new(TransitionIntent::None as u8),
            reason: RwLock::new(String::new()),
            last_seen_ts: AtomicU64::new(now_secs()),
            last_crash_ts: AtomicU64::new(0),
            crash_count: AtomicU32::new(0),
            restart_at: AtomicU64::new(0),
            cancel_tx: RwLock::new(None),
            current_registry_id: parking_lot::Mutex::new(None),
        });

        self.processes.write().insert(id, Arc::clone(&proc));
        self.notify_change(&proc);

        // Immediate spawn — don't wait for the tick.
        let sup = Arc::clone(self);
        tokio::spawn(async move {
            sup.begin_spawn(proc).await;
        });

        id
    }

    /// Bump `last_seen_ts` to now. Managers call this on every heartbeat
    /// or keepalive from the remote end of the bridge — channel plugins
    /// on `_va/heartbeat`, ACP agents on any notification, etc.
    pub fn touch(&self, id: ProcessId) {
        if let Some(proc) = self.processes.read().get(&id).cloned() {
            proc.last_seen_ts.store(now_secs(), Ordering::Relaxed);
        }
    }

    /// Stop the process. Cancels the current bridge and leaves the process
    /// in `Stopped` — no respawn regardless of policy.
    pub async fn force_stop(&self, id: ProcessId) -> ProcessResult<()> {
        let proc = self.get_proc(id)?;
        proc.intent
            .store(TransitionIntent::Stop as u8, Ordering::Release);
        self.cancel_current_bridge(&proc);
        Ok(())
    }

    /// Cancel the current bridge and force an immediate respawn (ignoring
    /// backoff). No-op if policy is `Never` and the process is already
    /// stopped.
    pub async fn force_restart(&self, id: ProcessId) -> ProcessResult<()> {
        let proc = self.get_proc(id)?;
        proc.intent
            .store(TransitionIntent::Restart as u8, Ordering::Release);
        self.cancel_current_bridge(&proc);
        Ok(())
    }

    /// If the process is `Stopped` / `Crashed` / `NotStarted`, schedule an
    /// immediate respawn. Ignored in `Running` / `Spawning`.
    pub fn force_start(&self, id: ProcessId) -> ProcessResult<()> {
        let proc = self.get_proc(id)?;
        match proc.status() {
            ProcessStatus::Stopped
            | ProcessStatus::Crashed
            | ProcessStatus::NotStarted => {
                proc.set_status(ProcessStatus::Crashed);
                proc.set_reason("started by user");
                proc.restart_at.store(now_secs(), Ordering::Relaxed);
                self.notify_change(&proc);
            }
            _ => {}
        }
        Ok(())
    }

    /// Snapshot of every registered process, sorted by label.
    pub fn snapshot(&self) -> Vec<ProcessSnapshot> {
        let now = now_secs();
        let mut out: Vec<_> = self
            .processes
            .read()
            .values()
            .map(|proc| {
                let last_seen = proc.last_seen_ts.load(Ordering::Relaxed);
                let restart_at = proc.restart_at.load(Ordering::Relaxed);
                ProcessSnapshot {
                    id: proc.id,
                    kind: proc.kind,
                    label: proc.label.clone(),
                    status: proc.status(),
                    reason: proc.reason.read().clone(),
                    crash_count: proc.crash_count.load(Ordering::Relaxed),
                    last_seen_age_secs: now.saturating_sub(last_seen),
                    restart_in_secs: restart_at.saturating_sub(now),
                }
            })
            .collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
        out
    }

    /// Subscribe to per-process status change events.
    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.change_tx.subscribe()
    }

    /// Start the supervisor's 5-second scan loop. Idempotent — a second
    /// call is a no-op. The loop runs for the process lifetime; daemon
    /// stop/restart cycles just drain the process table via
    /// [`shutdown_all`].
    ///
    /// [`shutdown_all`]: Supervisor::shutdown_all
    pub fn spawn_tick_loop(self: &Arc<Self>) {
        let mut started = self.tick_loop_started.lock();
        if *started {
            return;
        }
        *started = true;
        drop(started);
        let sup = Arc::clone(self);
        tokio::spawn(async move {
            sup.run_tick_loop().await;
        });
    }

    /// Cancel every active bridge and drain the process table so a
    /// subsequent daemon start gets a clean slate. The tick loop keeps
    /// running — it's process-wide and survives daemon restart.
    /// `ChildRegistry::kill_all()` is the hard-kill safety net from
    /// `RunningDaemon::stop`.
    pub async fn shutdown_all(&self) {
        let procs: Vec<Arc<SupervisedProcess>> = self
            .processes
            .write()
            .drain()
            .map(|(_, proc)| proc)
            .collect();
        for proc in procs {
            proc.intent
                .store(TransitionIntent::Stop as u8, Ordering::Release);
            self.cancel_current_bridge(&proc);
        }
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    fn get_proc(&self, id: ProcessId) -> ProcessResult<Arc<SupervisedProcess>> {
        self.processes.read().get(&id).cloned().ok_or_else(|| {
            ProcessError::UnknownProcess {
                label: format!("#{}", id.0),
            }
        })
    }

    fn cancel_current_bridge(&self, proc: &SupervisedProcess) {
        if let Some(tx) = proc.cancel_tx.read().as_ref() {
            let _ = tx.send(true);
        }
    }

    fn notify_change(&self, proc: &SupervisedProcess) {
        let _ = self.change_tx.send(ProcessEvent {
            id: proc.id,
            kind: proc.kind,
            status: proc.status(),
        });
    }

    async fn run_tick_loop(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // consume the immediate first tick

        tracing::info!(tick_secs = TICK_INTERVAL.as_secs(), "supervisor loop started");
        loop {
            ticker.tick().await;
            self.tick().await;
        }
    }

    async fn tick(self: &Arc<Self>) {
        let now = now_secs();
        let mut to_spawn: Vec<Arc<SupervisedProcess>> = Vec::new();
        let mut to_watchdog: Vec<Arc<SupervisedProcess>> = Vec::new();

        for proc in self.processes.read().values().cloned() {
            match proc.status() {
                ProcessStatus::NotStarted => to_spawn.push(proc),
                ProcessStatus::Crashed => {
                    let at = proc.restart_at.load(Ordering::Relaxed);
                    if at != 0 && now >= at {
                        to_spawn.push(proc);
                    }
                }
                ProcessStatus::Running => {
                    if let Some(watchdog) = proc.policy.watchdog() {
                        let last = proc.last_seen_ts.load(Ordering::Relaxed);
                        if now.saturating_sub(last) > watchdog.as_secs() {
                            to_watchdog.push(proc);
                        }
                    }
                }
                ProcessStatus::Spawning | ProcessStatus::Stopped => {}
            }
        }

        for proc in to_watchdog {
            let age = now.saturating_sub(proc.last_seen_ts.load(Ordering::Relaxed));
            proc_log!(
                info,
                kind = proc.kind,
                label = proc.label,
                event = "watchdog_fired",
                last_seen_age_secs = age
            );
            self.cancel_current_bridge(&proc);
        }

        for proc in to_spawn {
            let sup = Arc::clone(self);
            tokio::spawn(async move {
                sup.begin_spawn(proc).await;
            });
        }
    }

    async fn begin_spawn(self: &Arc<Self>, proc: Arc<SupervisedProcess>) {
        // Guard against racing tickers / immediate-spawn.
        let prev = proc
            .status
            .swap(ProcessStatus::Spawning as u8, Ordering::AcqRel);
        if matches!(ProcessStatus::from_u8(prev), ProcessStatus::Spawning) {
            return;
        }
        proc.restart_at.store(0, Ordering::Relaxed);
        proc.set_reason("spawning");
        self.notify_change(&proc);

        match self.spawn_child(&proc).await {
            Ok((pipes, cancel_rx)) => {
                // Publish status Running unless force_* landed during the spawn.
                if proc
                    .status
                    .compare_exchange(
                        ProcessStatus::Spawning as u8,
                        ProcessStatus::Running as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_err()
                {
                    proc_log!(
                        info,
                        kind = proc.kind,
                        label = proc.label,
                        event = "spawn_superseded"
                    );
                    // Cancel the bridge we just staged; the task spawned below
                    // will observe Cancelled and clean up.
                    self.cancel_current_bridge(&proc);
                    return;
                }
                proc.set_reason("");
                proc.last_seen_ts.store(now_secs(), Ordering::Relaxed);
                proc_log!(
                    info,
                    kind = proc.kind,
                    label = proc.label,
                    event = "running"
                );
                self.notify_change(&proc);

                // Hand pipes to the bridge in a task; when it returns, we
                // transition based on intent.
                let bridge = (proc.factory)();
                let sup = Arc::clone(self);
                let proc_for_task = Arc::clone(&proc);
                tokio::spawn(async move {
                    let exit = bridge.run(pipes, cancel_rx).await;
                    sup.handle_bridge_exit(proc_for_task, exit).await;
                });
            }
            Err(e) => {
                let reason = format!("{}", e);
                if proc
                    .status
                    .compare_exchange(
                        ProcessStatus::Spawning as u8,
                        ProcessStatus::Crashed as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_err()
                {
                    proc_log!(
                        error,
                        kind = proc.kind,
                        label = proc.label,
                        event = "spawn_failed_superseded",
                        error = %reason
                    );
                    return;
                }
                proc.set_reason(format!("spawn failed: {}", reason));
                proc.crash_count.fetch_add(1, Ordering::Relaxed);
                proc.last_crash_ts.store(now_secs(), Ordering::Relaxed);
                if let Some(backoff) = proc.policy.backoff() {
                    proc.restart_at
                        .store(now_secs() + backoff.as_secs(), Ordering::Relaxed);
                }
                proc_log!(
                    error,
                    kind = proc.kind,
                    label = proc.label,
                    event = "spawn_failed",
                    error = %reason
                );
                self.notify_change(&proc);
            }
        }
    }

    async fn spawn_child(
        &self,
        proc: &SupervisedProcess,
    ) -> ProcessResult<(StdioPipes, watch::Receiver<bool>)> {
        let mut cmd: Command = env::command(&proc.spec.program);
        cmd.args(&proc.spec.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &proc.spec.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &proc.spec.extra_env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| ProcessError::Spawn {
            program: proc.spec.program.clone(),
            source: e,
        })?;

        let stdin = child.stdin.take().ok_or(ProcessError::StdioUnavailable {
            what: "stdin",
        })?;
        let stdout = child.stdout.take().ok_or(ProcessError::StdioUnavailable {
            what: "stdout",
        })?;
        let stderr_raw = child.stderr.take().ok_or(ProcessError::StdioUnavailable {
            what: "stderr",
        })?;
        let pid = child.id();

        // Hand ownership of the Child to the global registry. This is the
        // canonical owner — kill_on_drop alone can't be relied on under
        // abrupt runtime teardown. The id is stashed on the proc so
        // `handle_bridge_exit` can remove + reap the child (otherwise
        // every respawn leaks a registry entry + zombie process).
        let registry_id = self
            .registry
            .register(proc.kind, proc.label.clone(), child);
        *proc.current_registry_id.lock() = Some(registry_id);

        proc_log!(
            info,
            kind = proc.kind,
            label = proc.label,
            pid = pid,
            event = "spawned",
            program = %proc.spec.program
        );

        let stderr = if proc.spec.capture_stderr {
            Some(stderr_raw)
        } else {
            let kind = proc.kind;
            let label = proc.label.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stderr_raw);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    proc_log!(
                        info,
                        kind = kind,
                        label = label,
                        event = "stderr",
                        line = %line
                    );
                }
            });
            None
        };

        let (cancel_tx, cancel_rx) = watch::channel(false);
        *proc.cancel_tx.write() = Some(cancel_tx);

        Ok((StdioPipes { stdin, stdout, stderr }, cancel_rx))
    }

    async fn handle_bridge_exit(
        self: Arc<Self>,
        proc: Arc<SupervisedProcess>,
        exit: BridgeExit,
    ) {
        // Clear the cancel channel — this run is done.
        *proc.cancel_tx.write() = None;

        // Pull the Child back out of the registry and reap it. Without
        // this, the registry accumulates an entry per respawn and the
        // process stays as a zombie on Unix until the daemon exits.
        // `kill_on_drop(true)` sends SIGKILL if the child is somehow
        // still alive; `wait()` then reaps. Reap on a background task so
        // the bridge-exit hot path isn't gated on a dying process.
        if let Some(id) = proc.current_registry_id.lock().take() {
            if let Some(mut child) = self.registry.remove(id) {
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }

        // Atomically consume intent so two callers don't both observe Stop.
        let intent = TransitionIntent::from_u8(
            proc.intent
                .swap(TransitionIntent::None as u8, Ordering::AcqRel),
        );

        let (reason, was_crash) = match exit {
            BridgeExit::Clean => ("clean exit".to_string(), false),
            BridgeExit::Cancelled => ("cancelled".to_string(), false),
            BridgeExit::ProtocolError(e) => (format!("protocol error: {}", e), true),
        };

        match intent {
            TransitionIntent::Stop => {
                proc.set_status(ProcessStatus::Stopped);
                proc.restart_at.store(0, Ordering::Relaxed);
                proc.set_reason(&reason);
                proc_log!(
                    info,
                    kind = proc.kind,
                    label = proc.label,
                    event = "stopped",
                    reason = %reason
                );
            }
            TransitionIntent::Restart => {
                proc.set_status(ProcessStatus::Crashed);
                proc.restart_at.store(now_secs(), Ordering::Relaxed);
                proc.set_reason(&reason);
                proc_log!(
                    info,
                    kind = proc.kind,
                    label = proc.label,
                    event = "restart_requested",
                    reason = %reason
                );
            }
            TransitionIntent::None => match proc.policy {
                RestartPolicy::Never => {
                    proc.set_status(ProcessStatus::Stopped);
                    proc.restart_at.store(0, Ordering::Relaxed);
                    proc.set_reason(&reason);
                    if was_crash {
                        proc.crash_count.fetch_add(1, Ordering::Relaxed);
                        proc.last_crash_ts.store(now_secs(), Ordering::Relaxed);
                        proc_log!(
                            warn,
                            kind = proc.kind,
                            label = proc.label,
                            event = "exited_no_restart",
                            reason = %reason
                        );
                    } else {
                        proc_log!(
                            info,
                            kind = proc.kind,
                            label = proc.label,
                            event = "exited",
                            reason = %reason
                        );
                    }
                }
                RestartPolicy::OnCrash { backoff, .. } => {
                    proc.set_status(ProcessStatus::Crashed);
                    proc.crash_count.fetch_add(1, Ordering::Relaxed);
                    proc.last_crash_ts.store(now_secs(), Ordering::Relaxed);
                    proc.restart_at
                        .store(now_secs() + backoff.as_secs(), Ordering::Relaxed);
                    proc.set_reason(&reason);
                    proc_log!(
                        warn,
                        kind = proc.kind,
                        label = proc.label,
                        event = "crashed",
                        reason = %reason,
                        respawn_in_secs = backoff.as_secs()
                    );
                }
            },
        }
        self.notify_change(&proc);

        // Auto-deregister terminal one-shot processes. Without this the
        // `processes` map grows unbounded over daemon lifetime as
        // `RestartPolicy::Never` workloads (chiefly `AcpAgent` spawns
        // tied to opened conversations) accumulate Stopped entries.
        // Keeping `OnCrash` entries around is deliberate: a user-stopped
        // channel plugin can still be resurrected via `force_start`.
        if matches!(proc.policy, RestartPolicy::Never)
            && matches!(proc.status(), ProcessStatus::Stopped)
        {
            self.processes.write().remove(&proc.id);
            proc_log!(
                info,
                kind = proc.kind,
                label = proc.label,
                event = "deregistered"
            );
        }
    }
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::bridge::{BridgeExit, CancelSignal, ProcessBridge, StdioPipes};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Bridge that waits for the cancel signal, then returns `Cancelled`.
    /// Drains stdout in the background so the child's pipe doesn't fill.
    struct WaitForCancelBridge;

    impl ProcessBridge for WaitForCancelBridge {
        fn run(
            self: Box<Self>,
            mut pipes: StdioPipes,
            mut cancel: CancelSignal,
        ) -> super::super::bridge::BridgeFuture {
            Box::pin(async move {
                // Drain stdout to keep `cat` happy.
                let drain = tokio::spawn(async move {
                    let mut buf = [0u8; 256];
                    loop {
                        match pipes.stdout.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                    }
                });
                // Kick one byte in so `cat` has something to echo; not essential.
                let _ = pipes.stdin.write_all(b".\n").await;
                let _ = cancel.wait_for(|v| *v).await;
                drop(pipes.stdin);
                let _ = drain.await;
                BridgeExit::Cancelled
            })
        }
    }

    /// Bridge that immediately returns `ProtocolError` — used to exercise
    /// the crash-then-backoff path without waiting for a real child to die.
    struct InstantErrorBridge;

    impl ProcessBridge for InstantErrorBridge {
        fn run(
            self: Box<Self>,
            _pipes: StdioPipes,
            _cancel: CancelSignal,
        ) -> super::super::bridge::BridgeFuture {
            Box::pin(async move {
                BridgeExit::ProtocolError(anyhow::anyhow!("synthetic failure"))
            })
        }
    }

    /// Bridge that immediately returns `Clean`.
    struct InstantCleanBridge;

    impl ProcessBridge for InstantCleanBridge {
        fn run(
            self: Box<Self>,
            _pipes: StdioPipes,
            _cancel: CancelSignal,
        ) -> super::super::bridge::BridgeFuture {
            Box::pin(async move { BridgeExit::Clean })
        }
    }

    async fn wait_for_status(sup: &Arc<Supervisor>, id: ProcessId, target: ProcessStatus) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let snap = sup.snapshot();
            if let Some(p) = snap.iter().find(|p| p.id == id) {
                if p.status == target {
                    return;
                }
            }
            if std::time::Instant::now() > deadline {
                let snap = sup.snapshot();
                panic!(
                    "timeout waiting for {:?}, got: {:?}",
                    target,
                    snap.iter().find(|p| p.id == id).map(|p| p.status)
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Wait until `id` disappears from the snapshot — i.e. auto-deregistered
    /// because it hit a terminal state under `RestartPolicy::Never`.
    async fn wait_for_absent(sup: &Arc<Supervisor>, id: ProcessId) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if !sup.snapshot().iter().any(|p| p.id == id) {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("timeout waiting for process {} to be deregistered", id);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    fn cat_spec() -> SpawnSpec {
        SpawnSpec::new("cat")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_runs_then_force_stop() {
        let registry = Arc::new(ChildRegistry::new());
        let sup = Supervisor::new(registry);

        let id = sup.register(
            ProcessKind::ChannelPlugin,
            "test-echo",
            cat_spec(),
            RestartPolicy::Never,
            Box::new(|| Box::new(WaitForCancelBridge)),
        );

        wait_for_status(&sup, id, ProcessStatus::Running).await;
        sup.force_stop(id).await.unwrap();
        // Never + Stopped auto-deregisters, so the snapshot entry vanishes.
        wait_for_absent(&sup, id).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn protocol_error_with_never_policy_deregisters() {
        let registry = Arc::new(ChildRegistry::new());
        let sup = Supervisor::new(registry);

        // Subscribe BEFORE register so we capture the transient Stopped
        // event (notify_change fires before auto-deregister).
        let mut rx = sup.subscribe();

        let id = sup.register(
            ProcessKind::AcpAgent,
            "test-fail",
            cat_spec(),
            RestartPolicy::Never,
            Box::new(|| Box::new(InstantErrorBridge)),
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut saw_stopped = false;
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(ev)) if ev.id == id && ev.status == ProcessStatus::Stopped => {
                    saw_stopped = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_stopped, "should have observed Stopped event");

        wait_for_absent(&sup, id).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_crash_policy_schedules_respawn() {
        let registry = Arc::new(ChildRegistry::new());
        let sup = Supervisor::new(registry);

        let id = sup.register(
            ProcessKind::ChannelPlugin,
            "test-crasher",
            cat_spec(),
            RestartPolicy::OnCrash {
                backoff: Duration::from_secs(30),
                watchdog: None,
            },
            Box::new(|| Box::new(InstantErrorBridge)),
        );

        wait_for_status(&sup, id, ProcessStatus::Crashed).await;
        let snap = sup.snapshot();
        let p = snap.iter().find(|p| p.id == id).unwrap();
        assert!(p.crash_count >= 1);
        assert!(p.restart_in_secs > 0, "backoff should schedule a future respawn");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_receives_events() {
        let registry = Arc::new(ChildRegistry::new());
        let sup = Supervisor::new(registry);
        let mut rx = sup.subscribe();

        let id = sup.register(
            ProcessKind::Tunnel,
            "test-events",
            cat_spec(),
            RestartPolicy::Never,
            Box::new(|| Box::new(InstantCleanBridge)),
        );

        // Expect at least one event where status == Stopped for this id.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut saw_stopped = false;
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(ev)) => {
                    if ev.id == id && ev.status == ProcessStatus::Stopped {
                        saw_stopped = true;
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(saw_stopped, "should have observed Stopped event");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_all_drains_but_keeps_loop_alive() {
        // Regression: `shutdown_all` used to take the tick-loop shutdown
        // sender and exit the loop, so after a daemon restart no new
        // OnCrash backoffs or watchdog checks would ever fire. The fix
        // keeps the loop alive for the process lifetime and only drains
        // the process table.
        let registry = Arc::new(ChildRegistry::new());
        let sup = Supervisor::new(registry);
        sup.spawn_tick_loop();

        let id = sup.register(
            ProcessKind::ChannelPlugin,
            "pre-shutdown",
            cat_spec(),
            RestartPolicy::Never,
            Box::new(|| Box::new(WaitForCancelBridge)),
        );
        wait_for_status(&sup, id, ProcessStatus::Running).await;

        sup.shutdown_all().await;
        wait_for_absent(&sup, id).await;

        // Post-shutdown register: the tick loop must still be alive to
        // drive this new process to Running.
        let id2 = sup.register(
            ProcessKind::ChannelPlugin,
            "post-shutdown",
            cat_spec(),
            RestartPolicy::Never,
            Box::new(|| Box::new(WaitForCancelBridge)),
        );
        wait_for_status(&sup, id2, ProcessStatus::Running).await;
        sup.force_stop(id2).await.unwrap();
        wait_for_absent(&sup, id2).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bridge_exit_removes_from_registry() {
        // Regression: the supervisor used to discard the registry_id
        // returned by ChildRegistry::register on every spawn, leaving
        // the entry and a zombie process behind on every respawn.
        let registry = Arc::new(ChildRegistry::new());
        let sup = Supervisor::new(Arc::clone(&registry));

        let id = sup.register(
            ProcessKind::AcpAgent,
            "reap-test",
            cat_spec(),
            RestartPolicy::Never,
            Box::new(|| Box::new(InstantCleanBridge)),
        );

        wait_for_absent(&sup, id).await;

        // Registry must be empty — otherwise every respawn leaks a Child
        // handle (and, on Unix, an unreaped zombie). The reap happens on
        // a background task so give it a beat to settle.
        for _ in 0..50 {
            if registry.len() == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("registry leaked {} entries after Never-policy exit", registry.len());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn factory_is_called_per_spawn() {
        let registry = Arc::new(ChildRegistry::new());
        let sup = Supervisor::new(registry);

        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);

        // Factory increments a counter each time it's called.
        let factory: BridgeFactory = Box::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
            Box::new(InstantCleanBridge)
        });

        let id = sup.register(
            ProcessKind::ChannelPlugin,
            "test-factory",
            cat_spec(),
            RestartPolicy::Never,
            factory,
        );

        // Never + Clean-exit auto-deregisters; by the time the entry is
        // gone, the factory has run exactly once.
        wait_for_absent(&sup, id).await;
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }
}
