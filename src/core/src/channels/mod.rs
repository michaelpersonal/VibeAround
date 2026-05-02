//! ACP-native channel manager: hosts channel plugins and routes traffic.
//!
//! The web channel path uses ACP directly (ws_chat dispatches via ConversationManager).
//! Stdio plugins still use the legacy ChannelInput/ChannelOutput for now.
//!
//! Module layout:
//! - `types`            — wire types: `ChannelEnvelope`, `ChannelInput`, `ChannelOutput`
//! - `slash`            — slash-command parser + `SlashAction` enum
//! - `prompt`           — `handle_channel_input` + `handle_prompt` + helpers
//! - `bridge_handler`   — `ChannelBridgeHandler` (notification + permission forwarding)
//! - `monitor`          — Dashboard-facing facade over `process::Supervisor`
//! - `plugin_bridge`    — `ChannelPluginBridge` (`ProcessBridge` impl for stdio plugins)
//! - `manifest`         — `ChannelPluginManifest`
//! - `plugin_host`      — runtime registry + pending permissions map
//! - `plugin_runtime`   — enum wrapper around Stdio / WebSocket runtimes
//! - `transport_stdio`  — ACP bridge to child plugin processes
//! - `transport_websocket` — in-process web chat channel

pub mod bridge_handler;
pub mod manifest;
pub mod monitor;
pub mod plugin_bridge;
pub mod plugin_host;
pub mod plugin_runtime;
pub mod prompt;
pub mod slash;
pub mod transport_stdio;
pub mod transport_websocket;
pub mod types;

use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{broadcast, mpsc};

use crate::conversations::event::SystemEvent;
use crate::conversations::ConversationManager;
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
    conversation_manager: Arc<ConversationManager>,
    /// Lazy-initialised on first `register_plugin` call. The monitor is
    /// a thin facade over `process::Supervisor` — it owns the supervisor
    /// and its tick loop internally.
    monitor: StdMutex<Option<Arc<monitor::ChannelMonitor>>>,
}

impl ChannelManager {
    pub fn new(conversation_manager: Arc<ConversationManager>) -> Self {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        Self {
            plugin_host: Arc::new(PluginHost::new(input_tx.clone())),
            input_tx,
            input_rx: StdMutex::new(Some(input_rx)),
            conversation_manager,
            monitor: StdMutex::new(None),
        }
    }

    pub fn plugin_host(&self) -> Arc<PluginHost> {
        Arc::clone(&self.plugin_host)
    }

    /// Return the monitor, initialising it on first call. Construction
    /// also spawns the underlying `process::Supervisor` tick loop.
    pub fn monitor(&self) -> Arc<monitor::ChannelMonitor> {
        let mut slot = self.monitor.lock().unwrap();
        if let Some(existing) = slot.as_ref() {
            return Arc::clone(existing);
        }
        let (change_tx, _) = tokio::sync::broadcast::channel::<()>(64);
        let m = monitor::ChannelMonitor::new(
            Arc::clone(&self.conversation_manager),
            self.input_tx.clone(),
            Arc::clone(&self.plugin_host),
            change_tx,
        );
        // Weak back-pointer so the plugin bridge's `_va/heartbeat`
        // handler can call `touch(kind)` on the monitor.
        self.plugin_host.set_monitor(Arc::downgrade(&m));

        *slot = Some(Arc::clone(&m));
        m
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
                    tracing::info!(
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
        crate::proc_log!(
            info,
            kind = crate::process::registry::ProcessKind::ChannelPlugin,
            label = channel_name,
            event = "registered_internal"
        );
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
        prompt::handle_channel_input(&self.conversation_manager, &self.plugin_host, input).await;
    }

    pub fn conversation_manager(&self) -> Arc<ConversationManager> {
        Arc::clone(&self.conversation_manager)
    }

    pub async fn send_output(&self, output: ChannelOutput) {
        self.plugin_host.send_output(output).await;
    }

    pub async fn shutdown_all(&self) {
        // Cancel every supervised plugin bridge first so they wind down
        // cleanly, then drop the host-side routing + pending permissions.
        let monitor = self.monitor.lock().unwrap().clone();
        if let Some(monitor) = monitor {
            monitor.shutdown_all().await;
        }
        self.plugin_host.shutdown_all().await;
    }

    /// Subscribe to ConversationManager `SystemEvent`s and forward relevant ones to
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
