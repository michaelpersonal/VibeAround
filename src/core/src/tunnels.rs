//! Tunnels module: expose the web dashboard over the internet via a public URL.
//! Each provider (Localtunnel, Ngrok, Cloudflare) implements TunnelBackend for unified management and dispatch.

use async_trait::async_trait;

pub mod manager;
mod providers;
pub mod status;

pub use manager::{TunnelInfo, TunnelManager};
pub use status::{TunnelMeta, TunnelStatus};

/// Tunnel provider: localtunnel (default), ngrok, or cloudflare.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)] // Ngrok/Cloudflare for future use
pub enum TunnelProvider {
    #[default]
    None,
    Localtunnel,
    Ngrok,
    Cloudflare,
}

impl TunnelProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            TunnelProvider::None => "none",
            TunnelProvider::Localtunnel => "localtunnel",
            TunnelProvider::Ngrok => "ngrok",
            TunnelProvider::Cloudflare => "cloudflare",
        }
    }

    /// Returns true if this provider actually creates a tunnel.
    pub fn is_enabled(&self) -> bool {
        !matches!(self, TunnelProvider::None)
    }

    /// Parse from config string (e.g. from settings.json "tunnel.provider").
    pub fn from_config(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "ngrok" => TunnelProvider::Ngrok,
            "cloudflare" => TunnelProvider::Cloudflare,
            "localtunnel" => TunnelProvider::Localtunnel,
            _ => TunnelProvider::None,
        }
    }

    /// Return the backend for this provider (for unified start_web_tunnel).
    fn backend(&self) -> Option<&'static dyn TunnelBackend> {
        match self {
            TunnelProvider::None => Option::None,
            TunnelProvider::Localtunnel => Some(&providers::localtunnel::LocaltunnelBackend),
            TunnelProvider::Ngrok => Some(&providers::ngrok::NgrokBackend),
            TunnelProvider::Cloudflare => Some(&providers::cloudflare::CloudflareBackend),
        }
    }
}

/// Unified tunnel backend trait: same interface for all providers so we can manage and dispatch uniformly.
#[async_trait]
pub trait TunnelBackend: Send + Sync {
    /// Provider id (e.g. "localtunnel", "ngrok") for config and logging.
    fn name(&self) -> &'static str;

    /// Start the web tunnel; config supplies credentials and options. Caller keeps the guard and awaits wait().
    async fn start_web_tunnel(
        &self,
        config: &crate::config::Config,
    ) -> Result<(TunnelGuard, String), Box<dyn std::error::Error + Send + Sync>>;
}

/// Guard that keeps the tunnel alive. Await `wait()` until the tunnel is done (e.g. process exit or SDK session closed).
pub enum TunnelGuard {
    /// Tunnel is a child process (e.g. localtunnel, or ngrok CLI).
    Process(tokio::process::Child),
    /// Tunnel is held by an SDK task (e.g. ngrok Rust SDK).
    Sdk(tokio::task::JoinHandle<()>),
}

impl TunnelGuard {
    /// Wait until the tunnel exits. For Process, waits for the child; for Sdk, waits for the background task.
    pub async fn wait(self) {
        match self {
            TunnelGuard::Process(mut child) => {
                let _ = child.wait().await;
            }
            TunnelGuard::Sdk(handle) => {
                let _ = handle.await;
            }
        }
    }
}

/// Start the web tunnel using the default provider (Localtunnel) and global config.
/// Returns (guard, public URL). Caller must keep the guard and await `guard.wait()` to keep the tunnel alive.
pub async fn start_web_tunnel() -> Result<(TunnelGuard, String), Box<dyn std::error::Error + Send + Sync>> {
    let config = crate::config::ensure_loaded();
    start_web_tunnel_with_provider(TunnelProvider::default(), &config).await
}

/// Start the web tunnel with the given provider and config (unified dispatch via TunnelBackend).
pub async fn start_web_tunnel_with_provider(
    provider: TunnelProvider,
    config: &crate::config::Config,
) -> Result<(TunnelGuard, String), Box<dyn std::error::Error + Send + Sync>> {
    match provider.backend() {
        Some(backend) => backend.start_web_tunnel(config).await,
        Option::None => Err("Tunnel provider is 'none' — no tunnel to start".into()),
    }
}

