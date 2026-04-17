//! ACP-native channel manager: hosts channel plugins and routes traffic.
//!
//! The web channel path uses ACP directly (ws_chat dispatches via ACPHub).
//! Stdio plugins still use the legacy ChannelInput/ChannelOutput for now.
//!
//! Module layout:
//! - `types`            — wire types: `ChannelEnvelope`, `ChannelInput`, `ChannelOutput`
//! - `slash`            — slash-command parser + `SlashAction` enum
//! - `prompt`           — `handle_channel_input` + `handle_prompt` + helpers
//! - `bridge_handler`   — `ChannelBridgeHandler` (notification + permission forwarding)
//! - `monitor`          — supervisor: respawn + heartbeat watchdog
//! - `manifest`         — `ChannelPluginManifest`
//! - `plugin_host`      — runtime registry + pending permissions map
//! - `plugin_runtime`   — enum wrapper around Stdio / WebSocket runtimes
//! - `transport_stdio`  — ACP bridge to child plugin processes
//! - `transport_websocket` — in-process web chat channel

pub mod bridge_handler;
pub mod manifest;
pub mod monitor;
pub mod plugin_host;
pub mod plugin_runtime;
pub mod prompt;
pub mod slash;
pub mod transport_stdio;
pub mod transport_websocket;
pub mod types;

use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{broadcast, mpsc};

use crate::acp_hub::event::SystemEvent;
use crate::acp_hub::ACPHub;
use crate::plugins::DiscoveredPlugin;

use self::manifest::ChannelPluginManifest;
use self::plugin_host::PluginHost;

// Re-exports so the rest of the crate keeps its existing import paths.
pub use self::prompt::handle_channel_input;
pub use self::transport_websocket::WebChannelManager;
pub use self::types::{ChannelEnvelope, ChannelInput, ChannelOutput};

/// Facade over the plugin host + monitor. Built once at daemon boot and
/// passed around as `Arc<ChannelManager>`.
pub struct ChannelManager {
    plugin_host: Arc<PluginHost>,
    /// Channel for fire-and-forget input dispatch.
    /// `handle_input` sends here; the processing loop runs on a dedicated
    /// `spawn_local` task so that `!Send` ACP futures are allowed.
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    input_rx: StdMutex<Option<mpsc::UnboundedReceiver<ChannelInput>>>,
    acp_hub: Arc<ACPHub>,
    /// Lazy-initialised on first `register_plugin` call. A single monitor
    /// task supervises every channel plugin (respawn + heartbeat watchdog).
    monitor: StdMutex<Option<Arc<monitor::ChannelMonitor>>>,
    /// Shutdown sender for the monitor loop. Kept here so `shutdown_all` can
    /// cleanly stop the monitor before tearing down runtimes.
    monitor_shutdown_tx: StdMutex<Option<mpsc::Sender<()>>>,
}

impl ChannelManager {
    pub fn new(acp_hub: Arc<ACPHub>) -> Self {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        Self {
            plugin_host: Arc::new(PluginHost::new(input_tx.clone())),
            input_tx,
            input_rx: StdMutex::new(Some(input_rx)),
            acp_hub,
            monitor: StdMutex::new(None),
            monitor_shutdown_tx: StdMutex::new(None),
        }
    }

    pub fn plugin_host(&self) -> Arc<PluginHost> {
        Arc::clone(&self.plugin_host)
    }

    /// Return the monitor, initialising it on first call. Also starts the
    /// monitor's tick loop as a top-level task.
    pub fn monitor(&self) -> Arc<monitor::ChannelMonitor> {
        let mut slot = self.monitor.lock().unwrap();
        if let Some(existing) = slot.as_ref() {
            return Arc::clone(existing);
        }
        let (change_tx, _) = tokio::sync::broadcast::channel::<()>(64);
        let m = monitor::ChannelMonitor::new(
            Arc::clone(&self.acp_hub),
            self.input_tx.clone(),
            Arc::clone(&self.plugin_host),
            change_tx,
        );
        // Establish the Weak back-pointer so bridge threads can call
        // mark_crashed / touch into the monitor.
        self.plugin_host.set_monitor(Arc::downgrade(&m));

        // Start the tick loop with a shutdown channel.
        let (sdn_tx, sdn_rx) = mpsc::channel::<()>(1);
        *self.monitor_shutdown_tx.lock().unwrap() = Some(sdn_tx);
        let m_clone = Arc::clone(&m);
        tokio::spawn(async move { monitor::run_monitor_loop(m_clone, sdn_rx).await });

        *slot = Some(Arc::clone(&m));
        m
    }

    /// Expose the monitor's change broadcast so `ServiceStatusManager` can
    /// unify dashboards into a single subscribe stream.
    pub fn monitor_change_tx(&self) -> Option<tokio::sync::broadcast::Sender<()>> {
        self.monitor
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.change_tx_clone())
    }

    /// Take the input receiver so the caller can drive the processing loop.
    /// Must be called exactly once (typically during daemon startup).
    pub fn take_input_rx(&self) -> Option<mpsc::UnboundedReceiver<ChannelInput>> {
        self.input_rx.lock().unwrap().take()
    }

    /// Register a channel plugin with the supervisor. The monitor spawns it
    /// immediately (without waiting for the next 5s tick) and keeps it alive
    /// via its respawn + watchdog loop.
    ///
    /// Returns `true` if the manifest was built and registered, `false` if
    /// the channel lacks config (plugin disabled).
    pub fn register_plugin(&self, channel_name: &str, plugin: &DiscoveredPlugin) -> bool {
        let manifest =
            match ChannelPluginManifest::from_discovered(channel_name.to_string(), plugin) {
                Some(manifest) => manifest,
                None => {
                    eprintln!(
                        "[{}] config=missing channels.{} — plugin disabled",
                        channel_name, channel_name
                    );
                    return false;
                }
            };
        self.monitor().register(manifest);
        true
    }

    pub fn start_internal_plugin(
        &self,
        channel_name: &str,
        outbound_tx: mpsc::UnboundedSender<ChannelOutput>,
    ) {
        self.plugin_host
            .register_websocket_plugin(channel_name.to_string(), outbound_tx);
        eprintln!("[{}] registered internal ACP plugin", channel_name);
    }

    /// Fire-and-forget: enqueue input for async processing. `Send`-safe
    /// because it only does a channel send.
    pub fn handle_input(&self, input: ChannelInput) {
        let _ = self.input_tx.send(input);
    }

    /// Process a single input on the current executor. May await `!Send` ACP
    /// futures — callers should run it on a `LocalSet` or other non-`Send`
    /// context.
    pub async fn process_input(&self, input: ChannelInput) {
        prompt::handle_channel_input(&self.acp_hub, &self.plugin_host, input).await;
    }

    pub fn acp_hub(&self) -> Arc<ACPHub> {
        Arc::clone(&self.acp_hub)
    }

    pub async fn send_output(&self, output: ChannelOutput) {
        self.plugin_host.send_output(output).await;
    }

    pub async fn shutdown_all(&self) {
        self.plugin_host.shutdown_all().await;
    }

    /// Subscribe to ACPHub `SystemEvent`s and forward relevant ones to
    /// channel plugins. Call once during daemon startup. Returns the
    /// forwarder task's `JoinHandle`.
    pub fn start_event_forwarder(
        &self,
        mut event_rx: broadcast::Receiver<SystemEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let plugin_host = Arc::clone(&self.plugin_host);
        tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => forward_system_event(&plugin_host, &event).await,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }
}

/// Translate lifecycle `SystemEvent`s into `ChannelOutput` for the plugin.
/// Currently surfaces agent-ready + session-ready so the IM user can see
/// which backend is speaking.
async fn forward_system_event(plugin_host: &Arc<PluginHost>, event: &SystemEvent) {
    match event {
        SystemEvent::AgentInitialized {
            route,
            cli_kind,
            initialize,
            ..
        } => {
            let agent_info = initialize.agent_info.as_ref();
            let agent = agent_info
                .map(|i| i.title.clone().unwrap_or_else(|| i.name.clone()))
                .or_else(|| cli_kind.clone())
                .unwrap_or_else(|| "agent".to_string());
            let version = agent_info.map(|i| i.version.clone()).unwrap_or_default();
            plugin_host
                .send_output(ChannelOutput::AgentReady {
                    route: route.clone(),
                    agent,
                    version,
                })
                .await;
        }
        SystemEvent::SessionReady { route, session_id } => {
            plugin_host
                .send_output(ChannelOutput::SessionReady {
                    route: route.clone(),
                    session_id: session_id.clone(),
                })
                .await;
        }
        _ => {}
    }
}
