//! `PluginRuntime` — the send-side polymorphism over the two kinds of
//! channel plugin the host talks to:
//!
//! - [`StdioPluginRuntime`] — a real `node` subprocess speaking ACP over
//!   stdio (feishu, slack, telegram, discord, …). `send_output` enqueues
//!   onto a `mpsc::UnboundedSender<ChannelOutput>`; the ACP serialization
//!   happens inside the bridge thread.
//! - [`WebSocketPluginRuntime`] — the in-process web dashboard channel.
//!   `send_output` pushes directly to the WS connection, no ACP involved.
//!
//! [`PluginHost::send_output`] uses this enum to dispatch without caring
//! about transport specifics. Lifecycle (spawn / kill / respawn) lives
//! in `process::Supervisor`; this enum only carries the send + shutdown
//! surfaces the host needs.
//!
//! [`PluginHost::send_output`]: super::plugin_host::PluginHost::send_output

use std::sync::Arc;

use super::transport_stdio::StdioPluginRuntime;
use super::transport_websocket::WebSocketPluginRuntime;
use super::ChannelOutput;

#[derive(Debug)]
pub enum PluginRuntime {
    Stdio(Arc<StdioPluginRuntime>),
    WebSocket(Arc<WebSocketPluginRuntime>),
}

impl PluginRuntime {
    pub async fn send_output(&self, output: ChannelOutput) {
        match self {
            Self::Stdio(runtime) => runtime.send_output(output).await,
            Self::WebSocket(runtime) => runtime.send_output(output).await,
        }
    }

    pub async fn shutdown(&self) {
        match self {
            Self::Stdio(runtime) => runtime.shutdown().await,
            Self::WebSocket(runtime) => runtime.shutdown().await,
        }
    }
}
