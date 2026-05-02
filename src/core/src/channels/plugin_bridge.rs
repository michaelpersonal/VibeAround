//! `ChannelPluginBridge` — the manager-owned `ProcessBridge` for stdio
//! channel plugins.
//!
//! On each (re)spawn, the [`process::Supervisor`] hands the bridge a fresh
//! [`StdioPipes`]. The bridge spins up a dedicated std::thread with a
//! `current_thread` tokio runtime + `LocalSet` — needed because the ACP
//! connection spawns `!Send` futures via `spawn_local`. Inside, it runs
//! [`run_acp_plugin_bridge`] to completion (or until the supervisor cancels
//! it) and forwards the resulting [`BridgeExit`] back to the supervisor
//! over a oneshot, so the overall [`ProcessBridge::run`] future stays
//! `Send`.
//!
//! [`process::Supervisor`]: crate::process::Supervisor
//! [`StdioPipes`]: crate::process::StdioPipes

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::conversations::ConversationManager;
use crate::process::bridge::{BridgeExit, BridgeFuture, CancelSignal, ProcessBridge, StdioPipes};

use super::plugin_host::PluginHost;
use super::transport_stdio::run_acp_plugin_bridge;
use super::{ChannelInput, ChannelOutput};

/// The per-spawn ACP bridge for a stdio channel plugin.
pub struct ChannelPluginBridge {
    pub channel_kind: String,
    pub raw_config: serde_json::Value,
    pub input_tx: mpsc::UnboundedSender<ChannelInput>,
    pub output_rx: mpsc::UnboundedReceiver<ChannelOutput>,
    pub conversation_manager: Arc<ConversationManager>,
    pub plugin_host: Arc<PluginHost>,
}

impl ProcessBridge for ChannelPluginBridge {
    fn run(self: Box<Self>, pipes: StdioPipes, cancel: CancelSignal) -> BridgeFuture {
        let this = *self;
        Box::pin(async move {
            let (exit_tx, exit_rx) = oneshot::channel::<BridgeExit>();
            let thread_name = format!("{}-plugin-bridge", this.channel_kind);

            let spawn_result = std::thread::Builder::new().name(thread_name).spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = exit_tx.send(BridgeExit::ProtocolError(anyhow::anyhow!(
                            "failed to build plugin bridge runtime: {}",
                            e
                        )));
                        return;
                    }
                };
                rt.block_on(async move {
                    let local = tokio::task::LocalSet::new();
                    local
                        .run_until(async move {
                            let exit = run_acp_plugin_bridge(
                                this.channel_kind,
                                this.raw_config,
                                pipes.stdin,
                                pipes.stdout,
                                this.input_tx,
                                this.output_rx,
                                this.conversation_manager,
                                this.plugin_host,
                                cancel,
                            )
                            .await;
                            let _ = exit_tx.send(exit);
                        })
                        .await;
                });
            });

            if let Err(e) = spawn_result {
                return BridgeExit::ProtocolError(anyhow::anyhow!(
                    "failed to spawn plugin bridge thread: {}",
                    e
                ));
            }

            // Bridge thread will send exactly one BridgeExit before exiting.
            // If the oneshot is dropped (bridge thread panicked before
            // sending), treat it as Clean — the supervisor will still log
            // the child's SIGPIPE / exit via ChildRegistry's tracking.
            exit_rx.await.unwrap_or(BridgeExit::Clean)
        })
    }
}
