//! `ServiceStatus` + `ServiceMeta` + `spawn_tracked`.
//!
//! The status enum itself, the common metadata block every registered
//! service carries (status + started_at + kill_fn), and the helper that
//! auto-marks a spawned task as `Stopped { reason: "completed" }` when its
//! future completes.

use std::sync::Arc;

use parking_lot::RwLock;
use tokio::task::AbortHandle;

use crate::pty::unix_now_secs;

/// Runtime status of a managed service.
#[derive(Debug, Clone)]
pub enum ServiceStatus {
    Running,
    Stopped { reason: String },
    Failed { error: String },
}

impl ServiceStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, ServiceStatus::Running)
    }
}

/// Common metadata shared by all service entry types.
pub struct ServiceMeta {
    pub status: Arc<RwLock<ServiceStatus>>,
    pub started_at: u64,
    /// Kill function — aborts the backing task.
    kill_fn: Option<Box<dyn Fn() + Send + Sync>>,
}

impl ServiceMeta {
    pub fn new(abort_handle: Option<AbortHandle>) -> Self {
        let kill_fn: Option<Box<dyn Fn() + Send + Sync>> =
            abort_handle.map(|h| Box::new(move || h.abort()) as Box<dyn Fn() + Send + Sync>);
        Self {
            status: Arc::new(RwLock::new(ServiceStatus::Running)),
            started_at: unix_now_secs(),
            kill_fn,
        }
    }

    pub fn current_status(&self) -> ServiceStatus {
        self.status.read().clone()
    }

    pub fn uptime_secs(&self) -> u64 {
        unix_now_secs().saturating_sub(self.started_at)
    }

    pub fn kill(&self) {
        if let Some(f) = &self.kill_fn {
            f();
        }
        // Never hold this write guard across an .await — we drop it at end of scope.
        let mut s = self.status.write();
        *s = ServiceStatus::Stopped {
            reason: "killed".into(),
        };
    }
}

/// Spawn a task that auto-updates the ServiceMeta status on completion.
pub fn spawn_tracked<F>(
    meta_status: Arc<RwLock<ServiceStatus>>,
    future: F,
) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let status = meta_status;
    tokio::spawn(async move {
        future.await;
        // The future has finished — we're past the last await. Safe to take
        // the blocking parking_lot write guard inside this async block.
        let mut s = status.write();
        if s.is_running() {
            *s = ServiceStatus::Stopped {
                reason: "completed".into(),
            };
        }
    })
}
