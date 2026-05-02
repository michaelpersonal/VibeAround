//! ACP bridge driver for a single stdio plugin.
//!
//! Owns the `AgentSideConnection` to the child and two concurrent tasks:
//! - The IO driver itself (`handle_io`).
//! - A `ChannelOutput` forwarder that drains `output_rx` and dispatches
//!   each variant to the corresponding ACP Client call.
//!
//! Returns a [`BridgeExit`] describing why the bridge ended. The
//! supervisor observes the exit and drives the state transition
//! (Running → Crashed / Stopped) according to policy + intent.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use agent_client_protocol as acp;

use crate::conversations::ConversationManager;
use crate::proc_log;
use crate::process::bridge::{BridgeExit, CancelSignal};
use crate::process::registry::ProcessKind;

use super::super::plugin_host::PluginHost;
use super::super::{ChannelInput, ChannelOutput};
use super::forwarder::forward_output_to_plugin;
use super::handler::PluginAgentHandler;

/// Run the ACP agent-side connection for a plugin to completion. Must be
/// called on a `LocalSet` — the ACP connection spawns `!Send` tasks via
/// `spawn_local`. Returns when the IO driver finishes or the cancel
/// signal fires.
pub(crate) async fn run_acp_plugin_bridge(
    channel_kind: String,
    config: serde_json::Value,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    mut output_rx: mpsc::UnboundedReceiver<ChannelOutput>,
    conversation_manager: Arc<ConversationManager>,
    plugin_host: Arc<PluginHost>,
    mut cancel: CancelSignal,
) -> BridgeExit {
    // Two clones: one moved into the handler, one moved into the
    // forwarder task, one kept here for `cancel_channel_permissions` at
    // the end of the bridge.
    let forwarder_plugin_host = Arc::clone(&plugin_host);
    let drain_plugin_host = Arc::clone(&plugin_host);
    let (conn, handle_io) = acp::AgentSideConnection::new(
        PluginAgentHandler::new(
            channel_kind.clone(),
            config.clone(),
            input_tx.clone(),
            conversation_manager,
            plugin_host,
        ),
        stdin.compat_write(),
        stdout.compat(),
        |fut| {
            tokio::task::spawn_local(fut);
        },
    );

    // Forward ChannelOutput → ACP Client methods. Spawned so it runs
    // concurrently with the IO driver below.
    let fwd_channel = channel_kind.clone();
    let forwarder = tokio::task::spawn_local(async move {
        proc_log!(
            info,
            kind = ProcessKind::ChannelPlugin,
            label = fwd_channel,
            event = "forwarder_started"
        );
        while let Some(output) = output_rx.recv().await {
            forward_output_to_plugin(&conn, &fwd_channel, &forwarder_plugin_host, output).await;
        }
        proc_log!(
            info,
            kind = ProcessKind::ChannelPlugin,
            label = fwd_channel,
            event = "forwarder_ended"
        );
    });

    let exit = tokio::select! {
        result = handle_io => match result {
            Ok(()) => {
                proc_log!(
                    info,
                    kind = ProcessKind::ChannelPlugin,
                    label = channel_kind,
                    event = "io_exited_clean"
                );
                BridgeExit::Clean
            }
            Err(error) => {
                proc_log!(
                    info,
                    kind = ProcessKind::ChannelPlugin,
                    label = channel_kind,
                    event = "io_terminated",
                    error = %error
                );
                BridgeExit::ProtocolError(anyhow::anyhow!(error))
            }
        },
        _ = cancel.wait_for(|v| *v) => {
            proc_log!(
                info,
                kind = ProcessKind::ChannelPlugin,
                label = channel_kind,
                event = "bridge_cancelled"
            );
            BridgeExit::Cancelled
        }
    };
    forwarder.abort();

    // Drain pending permission senders for this channel — otherwise any
    // `ChannelBridgeHandler::request_permission` caller blocked on a
    // reply from the dying plugin stalls forever. Previously invoked by
    // the old `ChannelMonitor::mark_crashed`; now lives here because the
    // supervisor is protocol-agnostic.
    drain_plugin_host.cancel_channel_permissions(&channel_kind);

    exit
}
