//! Cross-platform child process registry for plugin + agent subprocesses.
//!
//! Solves two related bugs:
//!
//! 1. **Orphaned plugin processes.** `tokio::process::Command::kill_on_drop(true)`
//!    only fires when the owning `Child` is actually Dropped. Our previous
//!    pattern moved `Child` into a `spawn_local` task that was aborted on
//!    shutdown — under abrupt runtime teardown, tokio does not necessarily
//!    poll that task to its destructor, so `kill_on_drop` never runs and the
//!    node plugin gets reparented to PID 1 (macOS/Linux) and keeps running.
//!
//! 2. **Orphaned ACP agent processes.** The agent factory moves `Child` into
//!    a `spawn_local` stdout-reader closure, dropping it only on stdout EOF.
//!    Same runtime-teardown hole as above.
//!
//! This module owns every spawned `Child` centrally in a `DashMap` behind
//! a global singleton so that:
//!
//! - `kill_all()` (called from `RunningDaemon::stop` and Tauri `RunEvent::Exit`)
//!   synchronously fires `start_kill()` on every child before the tokio
//!   runtime shuts down, guaranteeing SIGKILL regardless of task-poll order.
//!
//! - `orphan_sweep()` runs at daemon startup and kills any leftover
//!   `node` processes whose command line references
//!   `/.vibearound/plugins/` or an ACP agent package, whose parent is
//!   either init (PID 1 on Unix) or no longer alive. This self-heals
//!   from crashes, `kill -9`, and abrupt laptop sleeps.
//!
//! The registry does **not** replace per-process lifecycle management —
//! task authors still `remove()` cleanly on the normal shutdown path.
//! This is a safety net, not the primary drop path.

use std::sync::OnceLock;

use dashmap::DashMap;
use tokio::process::Child;

/// Classification of a registered child, used by `orphan_sweep` to decide
/// whether a leftover process belongs to us, and by the `Supervisor` for
/// structured logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessKind {
    /// Channel plugin process (node running under ~/.vibearound/plugins/).
    ChannelPlugin,
    /// ACP coding-agent child (node running the ACP bridge package).
    AcpAgent,
    /// PTY-hosted interactive shell or CLI tool.
    Pty,
    /// Tunnel provider subprocess (cloudflared, lt, …). Not ngrok (SDK).
    Tunnel,
}

impl ProcessKind {
    /// Short lowercase tag used in structured logs (`kind=channel_plugin`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ProcessKind::ChannelPlugin => "channel_plugin",
            ProcessKind::AcpAgent => "acp_agent",
            ProcessKind::Pty => "pty",
            ProcessKind::Tunnel => "tunnel",
        }
    }
}

#[derive(Debug)]
struct Entry {
    kind: ProcessKind,
    label: String,
    child: Child,
}

pub struct ChildRegistry {
    entries: DashMap<u64, Entry>,
    next_id: parking_lot::Mutex<u64>,
}

impl ChildRegistry {
    /// Number of live entries. Crate-private — used by `Supervisor` tests
    /// that assert the registry gets drained on terminal bridge exits.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn new() -> Self {
        Self {
            entries: DashMap::new(),
            next_id: parking_lot::Mutex::new(1),
        }
    }

    /// Global singleton. Stable across daemon restarts within the same
    /// process. Returned as `Arc<Self>` so the `Supervisor` can hold it
    /// through dependency injection while legacy callers still use
    /// `ChildRegistry::global().kill_all()` via `Arc` auto-deref.
    pub fn global() -> std::sync::Arc<ChildRegistry> {
        static INSTANCE: OnceLock<std::sync::Arc<ChildRegistry>> = OnceLock::new();
        std::sync::Arc::clone(INSTANCE.get_or_init(|| std::sync::Arc::new(ChildRegistry::new())))
    }

    /// Register a spawned child. Returns an opaque token that the caller
    /// must pass to `remove()` when the child exits cleanly.
    pub fn register(&self, kind: ProcessKind, label: impl Into<String>, child: Child) -> u64 {
        let label = label.into();
        let id = {
            let mut next = self.next_id.lock();
            let id = *next;
            *next = next.wrapping_add(1);
            id
        };
        let pid = child.id();
        tracing::info!(
            "[child-registry] register id={} kind={:?} label={} pid={:?}",
            id, kind, label, pid
        );
        self.entries.insert(id, Entry { kind, label, child });
        id
    }

    /// Remove a registered child and return its `Child` handle for graceful
    /// cleanup. Typically called after the caller has observed the child's
    /// stdout EOF, or during the happy-path shutdown.
    pub fn remove(&self, id: u64) -> Option<Child> {
        self.entries.remove(&id).map(|(_, entry)| {
            tracing::info!(
                "[child-registry] remove id={} kind={:?} label={}",
                id, entry.kind, entry.label
            );
            entry.child
        })
    }

    /// Synchronously fire `start_kill()` on every registered child. This
    /// sends SIGKILL on Unix and TerminateProcess on Windows. Safe to call
    /// from inside a tokio runtime OR from a Tauri `RunEvent::Exit` handler
    /// where no runtime is available.
    ///
    /// Intentionally does NOT `.await` on `wait()` — Exit handlers must
    /// return promptly, and the OS will reap the killed children anyway.
    pub fn kill_all(&self) {
        let ids: Vec<u64> = self.entries.iter().map(|e| *e.key()).collect();
        tracing::info!("[child-registry] kill_all: {} child(ren)", ids.len());
        for id in ids {
            if let Some((_, mut entry)) = self.entries.remove(&id) {
                let pid = entry.child.id();
                match entry.child.start_kill() {
                    Ok(()) => tracing::info!(
                        "[child-registry] killed id={} kind={:?} label={} pid={:?}",
                        id, entry.kind, entry.label, pid
                    ),
                    Err(e) => tracing::info!(
                        "[child-registry] start_kill failed id={} label={}: {}",
                        id, entry.label, e
                    ),
                }
            }
        }
    }
}

/// Sweep stale child processes left over from a previous crash.
///
/// Matches any `node` process whose command line contains either
/// `/.vibearound/plugins/` (channel plugins) or a known ACP package name
/// (`@agentclientprotocol/`, `@zed-industries/claude-code-acp`, etc.), AND
/// whose parent process is either init (PID 1) or no longer alive. Kills
/// them via SIGKILL.
///
/// Called at daemon startup BEFORE spawning any new children, so we don't
/// compete with our own fresh processes.
///
/// Cross-platform: `sysinfo` handles process enumeration on macOS, Linux,
/// and Windows. On Windows there is no PPID==1 invariant, so we instead
/// check whether the parent PID still maps to a live process.
pub fn orphan_sweep() {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let my_pid = std::process::id();
    let mut killed = 0usize;

    for (pid, proc_) in sys.processes() {
        if pid.as_u32() == my_pid {
            continue;
        }

        // Only consider `node` processes — VibeAround's subprocesses are all
        // node right now. Tighten if we ever spawn other runtimes.
        let name = proc_.name().to_string_lossy().to_lowercase();
        if !name.contains("node") {
            continue;
        }

        // Build full command line for pattern matching.
        let cmdline: String = proc_
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");

        let is_plugin = cmdline.contains("/.vibearound/plugins/")
            || cmdline.contains("\\.vibearound\\plugins\\");
        let is_agent_acp = cmdline.contains("@agentclientprotocol/")
            || cmdline.contains("@zed-industries/claude-code-acp")
            || cmdline.contains("claude-agent-acp")
            || cmdline.contains("gemini-acp")
            || cmdline.contains("qwen-code-acp");

        if !is_plugin && !is_agent_acp {
            continue;
        }

        // Orphan criterion:
        //   Unix: parent == 1 (reparented to init) OR parent missing
        //   Windows: parent missing from the process table
        let is_orphan = match proc_.parent() {
            None => true,
            Some(ppid) => {
                let ppid_u32 = ppid.as_u32();
                if cfg!(unix) && ppid_u32 == 1 {
                    true
                } else {
                    // Parent still listed? not orphan.
                    sys.process(Pid::from_u32(ppid_u32)).is_none()
                }
            }
        };

        if !is_orphan {
            continue;
        }

        tracing::info!(
            "[child-registry] orphan_sweep: killing pid={} ppid={:?} kind={} cmd={}",
            pid.as_u32(),
            proc_.parent().map(|p| p.as_u32()),
            if is_plugin { "plugin" } else { "agent-acp" },
            cmdline
        );

        if proc_.kill() {
            killed += 1;
        } else {
            tracing::info!(
                "[child-registry] orphan_sweep: failed to kill pid={}",
                pid.as_u32()
            );
        }
    }

    if killed > 0 {
        tracing::info!("[child-registry] orphan_sweep: killed {} orphan(s)", killed);
    }
}
