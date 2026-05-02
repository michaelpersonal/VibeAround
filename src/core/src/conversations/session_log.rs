//! Append-only indexes for conversation/session lifecycle events.
//!
//! This intentionally records only the IM route -> agent session mapping.
//! Full prompt/response history remains owned by each upstream agent.

use std::io;
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::routing::RouteKey;

const SCHEMA_VERSION: u8 = 1;
const EVENT_IM_SESSION_STARTED: &str = "im_session_started";

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionStartSource {
    NewSession,
    Pickup,
    StartupSession,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ImSessionStartRecord {
    schema_version: u8,
    event: &'static str,
    created_at: String,
    route: RouteKey,
    agent: ImSessionAgent,
    session: ImSession,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ImSessionAgent {
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ImSession {
    id: String,
    source: SessionStartSource,
}

impl ImSessionStartRecord {
    pub(crate) fn new(
        route: RouteKey,
        agent_kind: Option<String>,
        profile: Option<String>,
        session_id: String,
        source: SessionStartSource,
        workspace: Option<String>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            event: EVENT_IM_SESSION_STARTED,
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            route,
            agent: ImSessionAgent {
                kind: agent_kind,
                profile,
            },
            session: ImSession {
                id: session_id,
                source,
            },
            workspace,
        }
    }
}

pub(crate) fn is_im_route(route: &RouteKey) -> bool {
    !matches!(route.channel_kind.as_str(), "web" | "ws")
}

pub(crate) async fn append_im_session_started(record: ImSessionStartRecord) -> io::Result<PathBuf> {
    let dir = crate::config::data_dir().join("sessions");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("im-sessions.jsonl");

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;
    let line = serde_json::to_string(&record).map_err(io::Error::other)?;
    file.write_all(line.as_bytes()).await?;
    file.write_all(b"\n").await?;
    file.flush().await?;
    crate::auth::set_owner_only(&path)?;

    Ok(path)
}
