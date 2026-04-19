//! Per-domain WebSocket handlers.
//!
//! Each endpoint mirrors the corresponding `GET /api/<domain>` HTTP
//! handler: on connect it immediately sends the current list, then
//! subscribes to the manager's `subscribe_changes()` receiver and
//! re-sends the list on every `()` ping. Client-side the usage is
//! "whatever you last received IS the state; replace your view on
//! each message".
//!
//! Factored behind a tiny generic helper (`run_ws_loop`) so the three
//! endpoints only differ in which manager they poll and which wire
//! shape they produce.

use std::future::Future;

use axum::extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
};
use axum::response::Response;
use common::state::StateSource;
use serde::Serialize;
use tokio::sync::broadcast;

use super::AppState;

// ---------------------------------------------------------------------------
// GET /ws/channels
// ---------------------------------------------------------------------------

pub async fn ws_channels_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        let monitor = state.channel_hub.monitor();
        let rx = monitor.subscribe_changes();
        run_ws_loop(socket, rx, "ws/channels", move || {
            let monitor = monitor.clone();
            async move { build_channels(&monitor).await }
        })
        .await;
    })
}

async fn build_channels(
    monitor: &common::channel_manager::monitor::ChannelMonitor,
) -> Vec<crate::api_types::ChannelRuntime> {
    use common::state::StateSource;
    monitor
        .list()
        .await
        .into_iter()
        .map(|s| crate::api_types::ChannelRuntime {
            kind: s.kind,
            status: s.status.as_str(),
            reason: if s.reason.is_empty() { None } else { Some(s.reason) },
            crash_count: s.crash_count,
            last_seen_age_secs: s.last_seen_age_secs,
            restart_in_secs: s.restart_in_secs,
            started_at: s.started_at,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// GET /ws/tunnels
// ---------------------------------------------------------------------------

pub async fn ws_tunnels_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        let tunnels = state.tunnels.clone();
        let rx = tunnels.subscribe_changes();
        run_ws_loop(socket, rx, "ws/tunnels", move || {
            let tunnels = tunnels.clone();
            async move { build_tunnels(&tunnels).await }
        })
        .await;
    })
}

async fn build_tunnels(
    manager: &common::tunnels::TunnelManager,
) -> Vec<crate::api_types::TunnelRuntime> {
    use common::state::StateSource;
    manager
        .list()
        .await
        .into_iter()
        .map(|t| crate::api_types::TunnelRuntime {
            provider: t.provider.as_str(),
            url: t.url,
            status: (&t.status).into(),
            uptime_secs: t.uptime_secs,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// GET /ws/agents/runtime
// ---------------------------------------------------------------------------

pub async fn ws_agents_runtime_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        let acp_hub = state.channel_hub.acp_hub();
        let rx = acp_hub.subscribe_changes();
        run_ws_loop(socket, rx, "ws/agents/runtime", move || {
            let acp_hub = acp_hub.clone();
            async move { build_agents_runtime(&acp_hub).await }
        })
        .await;
    })
}

async fn build_agents_runtime(
    acp_hub: &common::acp_hub::ACPHub,
) -> Vec<crate::api_types::AgentRuntime> {
    let pods = acp_hub.list();
    let mut out = Vec::with_capacity(pods.len());
    for pod in pods {
        let st = pod.state().await;
        let (agent_name, agent_title, agent_version) = st
            .initialize
            .as_ref()
            .and_then(|i| i.agent_info.as_ref())
            .map(|info| {
                (
                    Some(info.name.clone()),
                    info.title.clone(),
                    Some(info.version.clone()),
                )
            })
            .unwrap_or((None, None, None));
        out.push(crate::api_types::AgentRuntime {
            route_key: pod.route.as_key(),
            channel_kind: pod.route.channel_kind.clone(),
            chat_id: pod.route.chat_id.clone(),
            cli_kind: st.cli_kind,
            profile: st.profile,
            session_id: st.session_id,
            workspace: st.workspace,
            busy: st.busy,
            failed: st.failed,
            started_at: pod.started_at(),
            agent_name,
            agent_title,
            agent_version,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Shared loop
// ---------------------------------------------------------------------------

/// Runs a WS session: first emit, then emit on every `()` ping from the
/// receiver. `build` produces the current list on demand; it's called
/// once at open and again on each change.
async fn run_ws_loop<T, F, Fut>(
    mut socket: WebSocket,
    mut rx: broadcast::Receiver<()>,
    label: &'static str,
    build: F,
) where
    T: Serialize,
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = T> + Send,
{
    if send_current(&mut socket, &build).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            result = rx.recv() => match result {
                Ok(()) => {
                    if send_current(&mut socket, &build).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("[VibeAround][{}] lagged by {}, sending fresh", label, n);
                    if send_current(&mut socket, &build).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(data))) => {
                    let _ = socket.send(Message::Pong(data)).await;
                }
                _ => {}
            },
        }
    }
}

async fn send_current<T, F, Fut>(socket: &mut WebSocket, build: &F) -> Result<(), ()>
where
    T: Serialize,
    F: Fn() -> Fut,
    Fut: Future<Output = T>,
{
    let data = build().await;
    let json = match serde_json::to_string(&data) {
        Ok(j) => j,
        Err(_) => return Ok(()), // skip malformed frames, don't close
    };
    if socket.send(Message::Text(json.into())).await.is_err() {
        return Err(());
    }
    Ok(())
}
