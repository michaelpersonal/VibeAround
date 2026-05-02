//! `PluginHost` — the per-daemon **routing table** for outbound channel
//! traffic, plus a small amount of bridge-adjacent bookkeeping.
//!
//! Three tables, one job each:
//!
//! 1. **`runtimes`** (`DashMap<ChannelKind, PluginRuntime>`) — "which live
//!    sender does a `ChannelOutput` for channel X go through?". The
//!    supervisor's bridge factory calls [`PluginHost::replace_stdio_runtime`]
//!    on every (re)spawn so the table always points at the live bridge;
//!    `ws_chat` calls [`PluginHost::register_websocket_plugin`] once per
//!    dashboard connection.
//!
//! 2. **`pending_permissions`** — in-flight `requestPermission` replies,
//!    keyed by a fresh `request_id`. The plugin-side forwarder pops from
//!    here when the plugin answers; [`PluginHost::cancel_channel_permissions`]
//!    drains the map when a plugin dies so waiting callers don't stall.
//!
//! 3. **`monitor: Weak<ChannelMonitor>`** — back-pointer used by the ACP
//!    bridge to report `_va/heartbeat` liveness. Weak to avoid a
//!    reference cycle (`ChannelMonitor` holds `Arc<PluginHost>`).
//!
//! `PluginHost` does **not** spawn processes, drive protocols, or own
//! state machines — those are `process::Supervisor`, the bridge threads,
//! and `ChannelMonitor` respectively.

use std::sync::{Arc, Weak};

use agent_client_protocol as acp;
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};

use crate::proc_log;
use crate::process::registry::ProcessKind;
use crate::routing::ChannelKind;

use super::monitor::ChannelMonitor;
use super::plugin_runtime::PluginRuntime;
use super::transport_stdio::StdioPluginRuntime;
use super::transport_websocket::WebSocketPluginRuntime;
use super::{ChannelInput, ChannelOutput};

pub struct PluginHost {
    runtimes: DashMap<ChannelKind, PluginRuntime>,
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    /// Pending `requestPermission` replies keyed by a fresh request_id.
    /// Value is `(channel_kind, sender)`: the sender is consumed by the
    /// plugin-bridge forwarder task once the plugin's ACP response arrives
    /// (see `transport_stdio::forwarder`), and the channel_kind lets us
    /// drain orphaned entries when that plugin dies
    /// (`cancel_channel_permissions`). Without the drain, a plugin crash
    /// during approval leaves the sender alive here forever and
    /// `request_permission`'s `rx.await` in `bridge_handler` stalls the
    /// upstream agent turn.
    pub pending_permissions:
        DashMap<String, (ChannelKind, oneshot::Sender<acp::RequestPermissionResponse>)>,
    /// Back-pointer to the ChannelMonitor. Weak to avoid a reference cycle
    /// (ChannelMonitor holds `Arc<PluginHost>`). Used by the plugin bridge
    /// to call `touch` on `_va/heartbeat`. `mark_crashed` is no longer
    /// needed here — the supervisor observes `BridgeExit` directly.
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

    /// Insert or replace the stdio runtime for a channel kind. Called by
    /// the supervisor's bridge factory on every (re)spawn so `send_output`
    /// always routes to the live process. Sync — the body is just a
    /// `DashMap::insert`.
    pub fn replace_stdio_runtime(
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
        proc_log!(
            debug,
            kind = ProcessKind::ChannelPlugin,
            label = route.channel_kind,
            event = "send_output",
            route = %route
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
            proc_log!(
                warn,
                kind = ProcessKind::ChannelPlugin,
                label = route.channel_kind,
                event = "no_runtime_for_route",
                route = %route,
                known = ?known
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
        // Drop every pending oneshot sender so waiting `request_permission`
        // callers in `ChannelBridgeHandler` see `rx.await -> Err` and fall
        // through to `Cancelled` instead of stalling forever.
        self.pending_permissions.clear();

        for runtime in runtimes {
            runtime.shutdown().await;
        }
    }

    /// Drop every pending permission request belonging to `channel_kind`.
    /// Called from `run_acp_plugin_bridge` right before it returns its
    /// `BridgeExit` — guaranteed to fire exactly once per bridge death.
    /// Without this drain, oneshot senders waiting on a reply from the
    /// dying plugin would stall `ChannelBridgeHandler::request_permission`
    /// callers indefinitely.
    pub fn cancel_channel_permissions(&self, channel_kind: &str) {
        let request_ids: Vec<String> = self
            .pending_permissions
            .iter()
            .filter(|entry| entry.value().0 == channel_kind)
            .map(|entry| entry.key().clone())
            .collect();
        for id in request_ids {
            self.pending_permissions.remove(&id);
        }
    }
}
