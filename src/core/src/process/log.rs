//! Structured log helper for supervised processes.
//!
//! Every log line from the supervisor and its bridges should go through
//! `proc_log!` so that `kind`, `label`, `pid`, and `event` are always
//! emitted as `tracing` fields — not concatenated into a message string.
//! This lets dashboards and `grep`-like tooling filter on any one field.

/// Emit a structured info-level log for a supervised process.
///
/// ```ignore
/// proc_log!(info, kind = ProcessKind::ChannelPlugin, label = "feishu", pid = 12345,
///           event = "spawned", "node {} started", entry.display());
/// ```
///
/// `kind` accepts anything with an `as_str()` → `&str` (i.e. `ProcessKind`).
/// Extra key=value pairs after the fixed fields are forwarded to `tracing`.
#[macro_export]
macro_rules! proc_log {
    ($level:ident, kind = $kind:expr, label = $label:expr, event = $event:expr $(, $($rest:tt)*)?) => {
        tracing::$level!(
            kind = $kind.as_str(),
            label = %$label,
            event = $event,
            $($($rest)*)?
        );
    };
    ($level:ident, kind = $kind:expr, label = $label:expr, pid = $pid:expr, event = $event:expr $(, $($rest:tt)*)?) => {
        tracing::$level!(
            kind = $kind.as_str(),
            label = %$label,
            pid = ?$pid,
            event = $event,
            $($($rest)*)?
        );
    };
}
