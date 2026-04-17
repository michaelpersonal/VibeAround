use std::sync::{Arc, Weak};

use agent_client_protocol as acp;
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};

use crate::acp::routing::ChannelKind;

use super::monitor::ChannelMonitor;
use super::plugin_runtime::PluginRuntime;
use super::transport_stdio::StdioPluginRuntime;
use super::transport_websocket::WebSocketPluginRuntime;
use super::{ChannelInput, ChannelOutput};

pub struct PluginHost {
    runtimes: DashMap<ChannelKind, PluginRuntime>,
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    /// Pending `requestPermission` replies keyed by a fresh request_id.
    /// The sender is consumed by the plugin-bridge forwarder task once the
    /// plugin's ACP response arrives. See `channel_manager::request_permission`.
    pub pending_permissions: DashMap<String, oneshot::Sender<acp::RequestPermissionResponse>>,
    /// Back-pointer to the ChannelMonitor. Weak to avoid a reference cycle
    /// (ChannelMonitor holds `Arc<PluginHost>`). Used by bridge threads to
    /// call `mark_crashed` on plugin exit and `touch` on `_va/heartbeat`.
    monitor: RwLock<Weak<ChannelMonitor>>,
}

impl PluginHost {
    pub fn new(input_tx: mpsc::UnboundedSender<ChannelInput>) -> Self {
        Self {
            runtimes: DashMap::new(),
            input_tx,
            pending_permissions: DashMap::new(),
            monitor: RwLock::new(Weak::new()),
        }
    }

    /// Called once at daemon boot after both `PluginHost` and `ChannelMonitor`
    /// exist. Establishes the back-pointer so bridge threads can signal the
    /// monitor.
    pub fn set_monitor(&self, monitor: Weak<ChannelMonitor>) {
        *self.monitor.write() = monitor;
    }

    pub fn monitor_weak(&self) -> Weak<ChannelMonitor> {
        self.monitor.read().clone()
    }

    pub fn input_tx(&self) -> mpsc::UnboundedSender<ChannelInput> {
        self.input_tx.clone()
    }

    /// Insert or replace the stdio runtime for a channel kind. Called by the
    /// monitor on initial spawn and on every respawn so `send_output` always
    /// routes to the live process.
    pub async fn replace_stdio_runtime(
        &self,
        channel_kind: &str,
        runtime: Arc<StdioPluginRuntime>,
    ) {
        self.runtimes
            .insert(channel_kind.to_string(), PluginRuntime::Stdio(runtime));
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
            let known: Vec<String> = self
                .runtimes
                .iter()
                .map(|e| format!("{:?}", e.key()))
                .collect();
            eprintln!(
                "[ChannelManager] no plugin runtime for route {} (looking up channel_kind={:?}, known={:?})",
                route, route.channel_kind, known
            );
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
