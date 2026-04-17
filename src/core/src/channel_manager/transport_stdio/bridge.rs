//! ACP bridge driver for a single stdio plugin.
//!
//! Owns the `AgentSideConnection` to the child, runs two concurrent tasks:
//! - The IO driver itself (`handle_io`).
//! - A `ChannelOutput` forwarder that drains `output_rx` and dispatches each
//!   variant to the corresponding ACP Client call.
//!
//! When `handle_io` returns (child stdio closed → child dead), we notify
//! the `ChannelMonitor` via `mark_crashed_weak` so it can respawn or mark
//! the channel Stopped depending on `TransitionIntent`.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use agent_client_protocol as acp;

use crate::acp_hub::ACPHub;

use super::super::plugin_host::PluginHost;
use super::super::{ChannelInput, ChannelOutput};
use super::forwarder::forward_output_to_plugin;
use super::handler::PluginAgentHandler;

/// Run the ACP agent-side connection on a dedicated thread.
/// Plugin is ACP Client, we are ACP Agent.
pub(super) async fn run_acp_plugin_bridge(
    channel_kind: String,
    config: serde_json::Value,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    mut output_rx: mpsc::UnboundedReceiver<ChannelOutput>,
    acp_hub: Arc<ACPHub>,
    plugin_host: Arc<PluginHost>,
) {
    let local = tokio::task::LocalSet::new();
    let fwd_plugin_host = Arc::clone(&plugin_host);
    local
        .run_until(async move {
            // Create ACP AgentSideConnection
            let (conn, handle_io) = acp::AgentSideConnection::new(
                PluginAgentHandler::new(
                    channel_kind.clone(),
                    config.clone(),
                    input_tx.clone(),
                    acp_hub,
                    plugin_host,
                ),
                stdin.compat_write(),
                stdout.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );

            // Forward ChannelOutput → ACP Client methods.
            // Spawn so it runs concurrently with handle_io below.
            let fwd_channel = channel_kind.clone();
            let monitor_notify_host = Arc::clone(&fwd_plugin_host);
            let forwarder = tokio::task::spawn_local(async move {
                eprintln!("[{}] output forwarder started", fwd_channel);
                while let Some(output) = output_rx.recv().await {
                    forward_output_to_plugin(&conn, &fwd_channel, &fwd_plugin_host, output).await;
                }
                eprintln!("[{}] output forwarder ended", fwd_channel);
            });

            // Drive ACP IO on this task. When the child plugin process dies
            // its stdin/stdout close, `handle_io` returns, and the LocalSet
            // exits — letting the block_on runtime drop, the std::thread
            // exit, and the bridge thread's resources release cleanly.
            let exit_reason = match handle_io.await {
                Err(error) => {
                    eprintln!("[{}] ACP plugin IO terminated: {}", channel_kind, error);
                    format!("io terminated: {}", error)
                }
                Ok(()) => {
                    eprintln!("[{}] ACP plugin bridge exited cleanly", channel_kind);
                    "bridge exited".to_string()
                }
            };
            forwarder.abort();

            // Notify the monitor so it can transition the channel out of
            // Running and schedule a respawn (unless the monitor observed a
            // user Stop intent, in which case it transitions to Stopped
            // instead). See `ChannelMonitor::mark_crashed`.
            super::super::monitor::mark_crashed_weak(
                &monitor_notify_host.monitor_weak(),
                &channel_kind,
                &exit_reason,
            );
        })
        .await;
}
