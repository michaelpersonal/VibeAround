//! VibeAround server crate: Axum HTTP + WebSocket, and the unified ServerDaemon entry point.

mod web_server;

pub use web_server::run_web_server;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use common::acp_hub::ACPHub;
use common::channel_manager::{handle_channel_input, ChannelManager, WebChannelManager};
use common::config;
use common::plugins;
use common::pty::{PtySessionManager, SessionId};
use common::runtime_status::RuntimeStatusStore;
use common::service::ServiceStatusManager;
use common::tunnels;

/// Unified daemon that starts and manages all VibeAround services.
/// Both the server binary and the desktop (Tauri) binary use this.
pub struct ServerDaemon {
    pub services: Arc<ServiceStatusManager>,
    pub port: u16,
}

pub struct RunningDaemon {
    pub channel_hub: Arc<ChannelManager>,
    pub acp_hub: Arc<ACPHub>,
    pub web_channel: Arc<WebChannelManager>,
    pub web_handle: JoinHandle<Result<(), String>>,
    pub tunnel_handle: JoinHandle<()>,
    pub web_dispatch_handle: JoinHandle<()>,
    pub services: Arc<ServiceStatusManager>,
}

impl RunningDaemon {
    pub async fn stop(&self) {
        self.acp_hub.shutdown_all().await;
        self.channel_hub.shutdown_all().await;

        let pty_manager = PtySessionManager::from_registry(Arc::clone(&self.services.pty));
        let session_ids: Vec<SessionId> = self.services.pty.iter().map(|entry| entry.key().clone()).collect();
        for session_id in session_ids {
            let _ = pty_manager.delete_session(session_id);
        }

        self.web_dispatch_handle.abort();
        self.web_handle.abort();
        self.tunnel_handle.abort();
    }
}

impl ServerDaemon {
    pub fn new(port: u16) -> Self {
        Self {
            services: Arc::new(ServiceStatusManager::new(port)),
            port,
        }
    }

    pub fn services(&self) -> Arc<ServiceStatusManager> {
        Arc::clone(&self.services)
    }

    pub async fn start_background(&self, dist_path: PathBuf) -> Result<RunningDaemon, String> {
        if let Ok(_) = tokio::net::TcpStream::connect(("127.0.0.1", self.port)).await {
            eprintln!(
                "[VibeAround] ⚠️  Another instance is already running on port {}. \
                 The new instance will fail to bind.",
                self.port
            );
        }

        let cfg = config::ensure_loaded();
        let services = Arc::clone(&self.services);

        // 1. Initialize hub architecture: ACPHub → ChannelManager
        let acp_hub = Arc::new(ACPHub::new());
        let channel_hub = Arc::new(ChannelManager::new(Arc::clone(&acp_hub)));
        let web_channel = WebChannelManager::new();

        // 2. Wire event subscribers: RuntimeStatusStore listens to SystemEvent broadcast
        let runtime_status = RuntimeStatusStore::new(services.change_tx());
        {
            let runtime_status = Arc::clone(&runtime_status);
            let mut event_rx = acp_hub.subscribe();
            tokio::spawn(async move {
                loop {
                    match event_rx.recv().await {
                        Ok(event) => runtime_status.project_event(&event),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
        services.set_runtime_status(Arc::clone(&runtime_status));

        // 3. ChannelManager subscribes to SystemEvent for agent info forwarding
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
        let mut input_rx = channel_hub.take_input_rx().expect("input_rx already taken");
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

        // 3. Channel plugins
        let discovered_plugins = plugins::discover_channel_plugins();
        for name in cfg.channel_names() {
            let Some(plugin) = discovered_plugins.get(&name) else {
                eprintln!("[VibeAround][daemon] no plugin found for channel '{}', skipping", name);
                continue;
            };
            if let Some(abort_handle) = channel_hub.start_plugin(&name, plugin).await {
                services.register_channel(&name, abort_handle);
            }
        }

        // 4. Web server (Axum)
        let web_services = Arc::clone(&services);
        let web_channel_hub = Arc::clone(&channel_hub);
        let web_channel_manager = Arc::clone(&web_channel);
        let web_handle = tokio::spawn(async move {
            run_web_server(
                common::config::DEFAULT_PORT,
                dist_path,
                web_services,
                web_channel_hub,
                web_channel_manager,
            )
            .await
            .map_err(|e| e.to_string())
        });

        // 5. Tunnel (skip when provider is "none")
        let tunnel_provider = cfg.tunnel_provider;
        eprintln!("[VibeAround][daemon] Tunnel ({})", tunnel_provider.as_str());
        let tunnel_handle = if tunnel_provider.is_enabled() {
            let tunnel_services = Arc::clone(&services);
            let handle = tokio::spawn(async move {
                match tunnels::start_web_tunnel_with_provider(tunnel_provider, &cfg).await {
                    Ok((guard, url)) => {
                        eprintln!("[VibeAround][daemon] Tunnel URL: {}", url);
                        tunnel_services.set_tunnel_url(tunnel_provider.as_str(), &url);
                        guard.wait().await;
                    }
                    Err(e) => {
                        eprintln!("[VibeAround][daemon] Tunnel failed: {}", e);
                    }
                }
            });
            services.register_tunnel(tunnel_provider, handle.abort_handle());
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
            services,
        })
    }

    pub async fn start(&self, dist_path: PathBuf) -> Result<(), String> {
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
