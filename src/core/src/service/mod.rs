//! Shared runtime status primitives.
//!
//! This module used to own a `ServiceStatusManager` facade that
//! aggregated tunnels / channels / agents / PTY into a single
//! `/api/services` snapshot. Phase 1g replaced that with per-domain
//! endpoints reading each manager directly (see `StateSource`), so the
//! facade is gone. What's left:
//!
//! - [`ServiceStatus`] — internal status enum for tunnel/agent entries.
//! - [`ServiceMeta`]   — runtime meta (status + started_at + abort).
//! - [`ApiServiceStatus`] — tagged wire enum still used by `TunnelRuntime`
//!   (and potentially future runtime surfaces).

mod snapshot;
mod status;

pub use snapshot::ApiServiceStatus;
pub use status::{ServiceMeta, ServiceStatus};
