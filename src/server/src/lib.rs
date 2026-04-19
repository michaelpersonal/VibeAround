//! VibeAround server crate: Axum HTTP + WebSocket, and the unified ServerDaemon entry point.

pub mod api_types;
mod web_server;

pub use web_server::run_web_server;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use tokio::task::JoinHandle;

use common::acp_hub::ACPHub;
use common::auth::{self, AuthToken};
use common::channel_manager::{handle_channel_input, ChannelManager, WebChannelManager};
use common::child_registry::{self, ChildRegistry};
use common::config;
use common::plugins;
use common::pty::{PtySessionManager, Registry, SessionId};
use common::tunnels::{self, TunnelManager};

/// Unified daemon that starts and manages all VibeAround services.
/// Both the server binary and the desktop (Tauri) binary use this.
pub struct ServerDaemon {
    pub tunnels: Arc<TunnelManager>,
    pub pty: Registry,
    pub port: u16,
    /// Per-session auth token, regenerated on every daemon start.
    /// Exposed so Tauri can append `?token=` when opening the dashboard.
    pub auth_token: Arc<AuthToken>,
}

pub struct RunningDaemon {
    pub channel_hub: Arc<ChannelManager>,
    pub acp_hub: Arc<ACPHub>,
    pub web_channel: Arc<WebChannelManager>,
    pub web_handle: JoinHandle<Result<(), String>>,
    pub tunnel_handle: JoinHandle<()>,
    pub web_dispatch_handle: JoinHandle<()>,
    pub tunnels: Arc<TunnelManager>,
    pub pty: Registry,
}

impl RunningDaemon {
    pub async fn stop(&self) {
        self.acp_hub.shutdown_all().await;
        self.channel_hub.shutdown_all().await;

        // Safety net: synchronously kill any child process still registered
        // after the graceful shutdown paths ran. Covers cases where guardian
        // tasks didn't get a chance to poll their drop handlers.
        ChildRegistry::global().kill_all();

        // Kill any user-started dev servers we were previewing so they don't
        // outlive the daemon. Best-effort; failures are logged.
        common::preview_entries::shutdown_kill_all_ports();

        let pty_manager = PtySessionManager::from_registry(Arc::clone(&self.pty));
        let session_ids: Vec<SessionId> = self.pty.iter().map(|entry| entry.key().clone()).collect();
        for session_id in session_ids {
            let _ = pty_manager.delete_session(session_id);
        }

        self.web_dispatch_handle.abort();
        self.web_handle.abort();
        self.tunnel_handle.abort();

        // Clear tunnel + PTY registries so stale entries don't persist
        // across restarts.
        self.tunnels.clear();
        self.pty.clear();
    }
}

impl ServerDaemon {
    pub fn new(port: u16) -> Self {
        Self {
            tunnels: TunnelManager::new(),
            pty: common::pty::new_registry(),
            port,
            auth_token: Arc::new(AuthToken::generate()),
        }
    }

    pub fn tunnels(&self) -> Arc<TunnelManager> {
        Arc::clone(&self.tunnels)
    }

    /// Borrow the session auth token. Tauri uses this to open the dashboard
    /// with a `?token=` query parameter.
    pub fn auth_token(&self) -> Arc<AuthToken> {
        Arc::clone(&self.auth_token)
    }

    /// Write the auth token file to `~/.vibearound/auth.json` so that
    /// out-of-process consumers (tray, cross-origin desktop-ui) can read
    /// the current token without an IPC round-trip.
    ///
    /// Safe to call before `start_background()` — the file will be
    /// overwritten there too, but the contents are identical, so the early
    /// write avoids a race where the desktop-ui queries the token before
    /// the daemon's start path has finished persisting it.
    pub fn persist_auth_token(&self) -> std::io::Result<()> {
        auth::write_token_file(self.port, &self.auth_token)
    }

    pub async fn start_background(&self, dist_path: PathBuf) -> anyhow::Result<RunningDaemon> {
        if tokio::net::TcpStream::connect(("127.0.0.1", self.port)).await.is_ok() {
            return Err(anyhow!(
                "Port {} is already in use — another VibeAround instance may be running",
                self.port
            ));
        }

        // Self-heal: kill any leftover plugin/agent-ACP node processes from
        // a previous crashed run BEFORE we spawn our own. Cheap on the happy
        // path (no matches) and prevents phantom children from hogging ports
        // or auth sockets.
        child_registry::orphan_sweep();

        // Force a fresh config read on every daemon start — ensures the
        // in-memory cache reflects the latest settings.json (which may have
        // been rewritten by onboarding or a manual edit since last start).
        let cfg = config::reload();
        let tunnels = Arc::clone(&self.tunnels);
        let pty = Arc::clone(&self.pty);

        // Persist the auth token so the Tauri side (tray, desktop-ui) can
        // read it without a separate IPC channel. Overwrites any stale file
        // from a previous run — older tokens are invalidated immediately.
        if let Err(e) = auth::write_token_file(self.port, &self.auth_token) {
            eprintln!(
                "[VibeAround][daemon] Failed to write auth token file: {} (the dashboard will reject requests without it)",
                e
            );
        }

        // 1. Initialize hub architecture: ACPHub → ChannelManager
        let acp_hub = Arc::new(ACPHub::new());
        let channel_hub = Arc::new(ChannelManager::new(Arc::clone(&acp_hub)));
        let web_channel = WebChannelManager::new();

        // 2. ChannelManager subscribes to SystemEvent for agent info forwarding
        channel_hub.start_event_forwarder(acp_hub.subscribe());

        // Register built-in internal channels.
        let (web_outbound_tx, mut web_outbound_rx) = web_channel.sender();
        channel_hub.start_internal_plugin("web", web_outbound_tx);
        let web_dispatch_handle = {
            let web_channel = Arc::clone(&web_channel);
            tokio::spawn(async move {
                while let Some(output) = web_outbound_rx.recv().await {
                    web_channel.dispatch_output(output);
                }
            })
        };

        // Start channel input processing loop on a dedicated thread with LocalSet.
        let mut input_rx = channel_hub.take_input_rx().context("input_rx already taken")?;
        let acp_hub_for_input = Arc::clone(&acp_hub);
        let plugin_host_for_input = channel_hub.plugin_host();
        std::thread::Builder::new()
            .name("channel-input".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to build input runtime");
                runtime.block_on(async move {
                    let local = tokio::task::LocalSet::new();
                    local.run_until(async move {
                        while let Some(input) = input_rx.recv().await {
                            let acp_hub = Arc::clone(&acp_hub_for_input);
                            let plugin_host = Arc::clone(&plugin_host_for_input);
                            tokio::task::spawn_local(async move {
                                handle_channel_input(&acp_hub, &plugin_host, input).await;
                            });
                        }
                    }).await;
                });
            })
            .expect("Failed to spawn channel input thread");

        // 3. Channel plugins — supervised by ChannelMonitor (respawn on
        //    crash + heartbeat watchdog). Handlers reach the monitor
        //    directly via `state.channel_hub.monitor()`; no back-ref
        //    needed.
        let discovered_plugins = plugins::discover_channel_plugins();
        for name in cfg.channel_names() {
            let Some(plugin) = discovered_plugins.get(&name) else {
                eprintln!("[VibeAround][daemon] no plugin found for channel '{}', skipping", name);
                continue;
            };
            channel_hub.register_plugin(&name, plugin);
        }

        // 4. Web server (Axum)
        let web_tunnels = Arc::clone(&tunnels);
        let web_pty = Arc::clone(&pty);
        let web_channel_hub = Arc::clone(&channel_hub);
        let web_channel_manager = Arc::clone(&web_channel);
        let web_auth_token = Arc::clone(&self.auth_token);
        let daemon_port = self.port;
        let web_handle = tokio::spawn(async move {
            run_web_server(
                daemon_port,
                dist_path,
                web_tunnels,
                web_pty,
                web_channel_hub,
                web_channel_manager,
                web_auth_token,
            )
            .await
            .map_err(|e| e.to_string())
        });

        // 5. Tunnel (skip when provider is "none")
        let tunnel_provider = cfg.tunnel_provider;
        eprintln!("[VibeAround][daemon] Tunnel ({})", tunnel_provider.as_str());
        let tunnel_handle = if tunnel_provider.is_enabled() {
            let tunnel_manager = Arc::clone(&tunnels);
            let handle = tokio::spawn(async move {
                match tunnels::start_web_tunnel_with_provider(tunnel_provider, &cfg).await {
                    Ok((guard, url)) => {
                        eprintln!("[VibeAround][daemon] Tunnel URL: {}", url);
                        tunnel_manager.set_url(tunnel_provider.as_str(), &url);
                        guard.wait().await;
                    }
                    Err(e) => {
                        eprintln!("[VibeAround][daemon] Tunnel failed: {}", e);
                    }
                }
            });
            tunnels.register(tunnel_provider, handle.abort_handle());
            handle
        } else {
            eprintln!("[VibeAround][daemon] Tunnel disabled (none)");
            tokio::spawn(async { /* no-op: keep the JoinHandle type consistent */ })
        };

        Ok(RunningDaemon {
            channel_hub,
            acp_hub,
            web_channel,
            web_handle,
            tunnel_handle,
            web_dispatch_handle,
            tunnels,
            pty,
        })
    }

    pub async fn start(&self, dist_path: PathBuf) -> anyhow::Result<()> {
        let mut running = self.start_background(dist_path).await?;

        tokio::select! {
            result = &mut running.web_handle => {
                match result {
                    Ok(Ok(())) => eprintln!("[VibeAround][daemon] web server stopped"),
                    Ok(Err(e)) => eprintln!("[VibeAround][daemon] web server error: {}", e),
                    Err(e) => eprintln!("[VibeAround][daemon] web server panic: {}", e),
                }
                running.tunnel_handle.abort();
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\n[VibeAround][daemon] shutting down...");
                running.stop().await;
            }
        }

        Ok(())
    }
}
