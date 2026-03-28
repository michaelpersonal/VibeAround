use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::acp::routing::ChannelKind;
use crate::acp_hub::ACPHub;

use super::manifest::ChannelPluginManifest;
use super::plugin_runtime::PluginRuntime;
use super::transport_stdio::StdioPluginRuntime;
use super::transport_websocket::WebSocketPluginRuntime;
use super::{ChannelInput, ChannelOutput};

pub struct PluginHost {
    runtimes: DashMap<ChannelKind, PluginRuntime>,
    input_tx: mpsc::UnboundedSender<ChannelInput>,
}

impl PluginHost {
    pub fn new(input_tx: mpsc::UnboundedSender<ChannelInput>) -> Self {
        Self {
            runtimes: DashMap::new(),
            input_tx,
        }
    }

    pub async fn register_stdio_plugin(
        &self,
        manifest: ChannelPluginManifest,
        acp_hub: Arc<ACPHub>,
        plugin_host: Arc<PluginHost>,
    ) -> Result<AbortHandle, String> {
        let channel_kind = manifest.channel_kind.clone();
        let runtime = Arc::new(
            StdioPluginRuntime::spawn(manifest, self.input_tx.clone(), acp_hub, plugin_host)
                .await?,
        );
        let abort_handle = runtime.abort_handle();
        self.runtimes
            .insert(channel_kind, PluginRuntime::Stdio(runtime));
        Ok(abort_handle)
    }

    pub fn register_websocket_plugin(
        &self,
        channel_kind: impl Into<ChannelKind>,
        outbound_tx: mpsc::UnboundedSender<ChannelOutput>,
    ) {
        let channel_kind = channel_kind.into();
        let runtime = WebSocketPluginRuntime::new(channel_kind.clone(), outbound_tx);
        self.runtimes
            .insert(channel_kind, PluginRuntime::WebSocket(runtime));
    }

    pub async fn send_output(&self, output: ChannelOutput) {
        let route = output.route_key().clone();
        eprintln!(
            "[PluginHost] send_output route={} channel_kind={}",
            route, route.channel_kind
        );
        let runtime = self
            .runtimes
            .get(&route.channel_kind)
            .map(|entry| match entry.value() {
                PluginRuntime::Stdio(runtime) => PluginRuntime::Stdio(Arc::clone(runtime)),
                PluginRuntime::WebSocket(runtime) => PluginRuntime::WebSocket(Arc::clone(runtime)),
            });

        if let Some(runtime) = runtime {
            runtime.send_output(output).await;
        } else {
            eprintln!("[ChannelManager] no plugin runtime for route {}", route);
        }
    }

    pub async fn shutdown_all(&self) {
        let runtimes: Vec<PluginRuntime> = self
            .runtimes
            .iter()
            .map(|entry| match entry.value() {
                PluginRuntime::Stdio(runtime) => PluginRuntime::Stdio(Arc::clone(runtime)),
                PluginRuntime::WebSocket(runtime) => PluginRuntime::WebSocket(Arc::clone(runtime)),
            })
            .collect();

        self.runtimes.clear();

        for runtime in runtimes {
            runtime.shutdown().await;
        }
    }
}
