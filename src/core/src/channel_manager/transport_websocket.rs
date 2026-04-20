use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;

use crate::routing::ChannelKind;

use super::ChannelOutput;

/// Outbound sink to a single web chat connection.
pub type WebChatSink = mpsc::UnboundedSender<ChannelOutput>;

/// Internal websocket-backed channel manager used by the browser chat UI.
pub struct WebChannelManager {
    connections: DashMap<String, WebChatSink>,
}

impl WebChannelManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            connections: DashMap::new(),
        })
    }

    pub fn register_connection(&self, chat_id: String, sink: WebChatSink) {
        self.connections.insert(chat_id, sink);
    }

    pub fn unregister_connection(&self, chat_id: &str) {
        self.connections.remove(chat_id);
    }

    pub fn sender(&self) -> (mpsc::UnboundedSender<ChannelOutput>, mpsc::UnboundedReceiver<ChannelOutput>) {
        mpsc::unbounded_channel()
    }

    pub fn dispatch_output(&self, output: ChannelOutput) {
        let chat_id = &output.route_key().chat_id;
        let has_conn = self.connections.contains_key(chat_id);
        tracing::info!(
            "[WebChannelManager] dispatch_output chat_id={} has_connection={}",
            chat_id, has_conn
        );
        if let Some(entry) = self.connections.get(chat_id) {
            let _ = entry.send(output);
        }
    }
}

#[derive(Debug)]
pub struct WebSocketPluginRuntime {
    channel_kind: ChannelKind,
    outbound_tx: mpsc::UnboundedSender<ChannelOutput>,
}

impl WebSocketPluginRuntime {
    pub fn new(
        channel_kind: impl Into<ChannelKind>,
        outbound_tx: mpsc::UnboundedSender<ChannelOutput>,
    ) -> Arc<Self> {
        Arc::new(Self {
            channel_kind: channel_kind.into(),
            outbound_tx,
        })
    }

    pub async fn send_output(&self, output: ChannelOutput) {
        tracing::info!(
            "[WebSocketPluginRuntime] send_output channel_kind={} route={}",
            self.channel_kind, output.route_key()
        );
        if let Err(error) = self.outbound_tx.send(output) {
            tracing::info!(
                "[{}] failed to deliver websocket output: {}",
                self.channel_kind, error
            );
        }
    }

    pub async fn shutdown(&self) {}
}
