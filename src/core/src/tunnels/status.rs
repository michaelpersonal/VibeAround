//! `TunnelStatus` + `TunnelMeta` — runtime state for a registered tunnel.
//!
//! `TunnelStatus` serializes as a tagged JSON object with a `state`
//! discriminant so consumers pattern-match exhaustively:
//!
//! ```json
//! { "state": "running" }
//! { "state": "stopped", "reason": "killed" }
//! { "state": "failed",  "error":  "spawn failed" }
//! ```
//!
//! Reference zod schema:
//! `src/shared/client-ts/src/schemas.ts::TunnelStatusSchema`.

use std::sync::Arc;

use parking_lot::RwLock;
use serde::Serialize;
use tokio::task::AbortHandle;

use crate::pty::unix_now_secs;

/// Runtime status of a tunnel. Wire-compatible via the `state` tag.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TunnelStatus {
    Running,
    Stopped { reason: String },
    Failed { error: String },
}

impl TunnelStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, TunnelStatus::Running)
    }
}

/// Runtime metadata attached to each tunnel entry: status, start
/// timestamp, and the abort closure used by `kill`.
pub struct TunnelMeta {
    pub status: Arc<RwLock<TunnelStatus>>,
    pub started_at: u64,
    kill_fn: Option<Box<dyn Fn() + Send + Sync>>,
}

impl TunnelMeta {
    pub fn new(abort_handle: Option<AbortHandle>) -> Self {
        let kill_fn: Option<Box<dyn Fn() + Send + Sync>> =
            abort_handle.map(|h| Box::new(move || h.abort()) as Box<dyn Fn() + Send + Sync>);
        Self {
            status: Arc::new(RwLock::new(TunnelStatus::Running)),
            started_at: unix_now_secs(),
            kill_fn,
        }
    }

    pub fn current_status(&self) -> TunnelStatus {
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
        *s = TunnelStatus::Stopped {
            reason: "killed".into(),
        };
    }
}
