//! Unified error type for the `process::Supervisor` boundary.
//!
//! Everything that spawns, kills, or talks to a supervised subprocess
//! funnels through this type. Upstream code still uses `anyhow` freely —
//! `ProcessError: Into<anyhow::Error>` so `?` works either way.

use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },

    #[error("child stdio unavailable: {what}")]
    StdioUnavailable { what: &'static str },

    #[error("bridge thread failed to start: {0}")]
    BridgeThread(#[source] io::Error),

    #[error("process {label} is not registered")]
    UnknownProcess { label: String },

    #[error("bridge protocol error: {0}")]
    Protocol(#[from] anyhow::Error),
}

pub type ProcessResult<T> = Result<T, ProcessError>;
