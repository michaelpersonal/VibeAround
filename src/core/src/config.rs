//! Config loading helpers.
//! All config comes from ~/.vibearound/settings.json.
//! Callers load a fresh Config when they need one.

use std::path::PathBuf;
use std::sync::{Arc, Once};

use parking_lot::RwLock;

use crate::tunnels::TunnelProvider;

/// Global config cache. Populated on first `ensure_loaded()` call, reloaded
/// by `reload()` or automatically after `update_settings_json()`.
static CONFIG_CACHE: RwLock<Option<Arc<Config>>> = RwLock::new(None);

/// Default server port for both standalone server and desktop-spawned server.
pub const DEFAULT_PORT: u16 = 12358;

/// Minimal default settings.json content, embedded at compile time.
const DEFAULT_SETTINGS_JSON: &str = r#"{
  "workspaces": [],
  "default_workspace": ""
}"#;

/// User home directory (HOME on Unix, USERPROFILE on Windows).
pub fn home_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/tmp".into()),
    )
}

/// Data directory: ~/.vibearound
pub fn data_dir() -> PathBuf {
    home_dir().join(".vibearound")
}

/// Ensure ~/.vibearound/ exists with settings.json and workspaces/.
fn init_data_dir() {
    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("[VibeAround] Failed to create data dir {:?}: {}", dir, e);
        return;
    }
    let settings_path = dir.join("settings.json");
    if !settings_path.exists() {
        eprintln!("[VibeAround] Creating default settings.json at {:?}", settings_path);
        if let Err(e) = std::fs::write(&settings_path, DEFAULT_SETTINGS_JSON) {
            eprintln!("[VibeAround] Failed to write settings.json: {}", e);
        } else if let Err(e) = crate::auth::set_owner_only(&settings_path) {
            eprintln!("[VibeAround] Failed to chmod settings.json: {}", e);
        }
    }
    let ws_dir = dir.join("workspaces");
    if let Err(e) = std::fs::create_dir_all(&ws_dir) {
        eprintln!("[VibeAround] Failed to create workspaces dir: {}", e);
    }
}

/// Install rustls default crypto provider once.
fn ensure_rustls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .expect("rustls default crypto provider");
    });
}

/// Per-channel verbose/output settings for IM.
#[derive(Debug, Clone)]
pub struct ImVerboseConfig {
    pub show_thinking: bool,
    pub show_tool_use: bool,
}

impl Default for ImVerboseConfig {
    fn default() -> Self {
        Self { show_thinking: false, show_tool_use: false }
    }
}

/// Cached config from settings.json.
#[derive(Clone)]
pub struct Config {
    // --- Tunnel ---
    pub tunnel_provider: TunnelProvider,
    pub ngrok_auth_token: Option<String>,
    pub ngrok_domain: Option<String>,
    pub cloudflare_tunnel_token: Option<String>,
    pub cloudflare_hostname: Option<String>,
    // --- Workspaces ---
    /// User-added project folders (not including the built-in ~/.vibearound/workspaces/).
    pub workspaces: Vec<PathBuf>,
    /// User override for default workspace. None = use built-in per-agent default.
    pub default_workspace: Option<PathBuf>,
    pub preview_base_url: Option<String>,
    pub tmux_detach_others: bool,
    // --- Agents ---
    pub default_agent: String,
    pub enabled_agents: Vec<crate::agent_factory::agents::AgentKind>,
    // --- Raw channels JSON (for dynamic plugin config) ---
    raw_channels: serde_json::Value,
}

impl Config {
    /// List all channel names configured in settings.json (e.g. ["feishu", "telegram"]).
    pub fn channel_names(&self) -> Vec<String> {
        self.raw_channels
            .as_object()
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Get raw JSON config for a specific channel (e.g. channels.feishu → { app_id, app_secret, ... }).
    /// Passed directly to the plugin process via initialize.
    pub fn channel_raw_config(&self, name: &str) -> Option<serde_json::Value> {
        self.raw_channels.get(name).cloned()
    }

    /// Get verbose config for a specific channel.
    pub fn channel_verbose(&self, name: &str) -> ImVerboseConfig {
        parse_verbose_config(self.raw_channels.get(name))
    }

    /// Resolve the workspace directory for an agent session.
    /// - If user set a default_workspace → use it directly
    /// - Otherwise → ~/.vibearound/workspaces/{agent_kind}-default
    pub fn resolve_workspace(&self, agent_kind: &str) -> PathBuf {
        if let Some(ref ws) = self.default_workspace {
            ws.clone()
        } else {
            let subdir = format!("{}-default", agent_kind);
            data_dir().join("workspaces").join(subdir)
        }
    }

    /// All available workspaces: the built-in root + user-added paths.
    pub fn all_workspaces(&self) -> Vec<PathBuf> {
        let mut all = vec![builtin_workspaces_dir()];
        for ws in &self.workspaces {
            if !all.contains(ws) {
                all.push(ws.clone());
            }
        }
        all
    }
}

/// Load config — returns cached version if available, otherwise reads from disk.
/// Call `reload()` to force a fresh read (e.g. after settings change).
pub fn ensure_loaded() -> Arc<Config> {
    // Fast path: return cached config.
    if let Some(cfg) = CONFIG_CACHE.read().as_ref() {
        return Arc::clone(cfg);
    }
    // Slow path: first call — initialize data dir, read from disk, cache.
    ensure_rustls_provider();
    init_data_dir();
    let path = data_dir().join("settings.json");
    let cfg = Arc::new(load_settings_from(&path));
    *CONFIG_CACHE.write() = Some(Arc::clone(&cfg));
    cfg
}

/// Force re-read config from disk and update the cache.
/// Called after `update_settings_json()` and on daemon restart.
pub fn reload() -> Arc<Config> {
    ensure_rustls_provider();
    init_data_dir();
    let path = data_dir().join("settings.json");
    let cfg = Arc::new(load_settings_from(&path));
    *CONFIG_CACHE.write() = Some(Arc::clone(&cfg));
    cfg
}

fn load_settings_from(path: &std::path::Path) -> Config {
    let Ok(data) = std::fs::read_to_string(path) else {
        return Config::default();
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&data) else {
        return Config::default();
    };

    let tunnel_provider = root
        .get("tunnel")
        .and_then(|t| t.get("provider"))
        .and_then(|p| p.as_str())
        .map(TunnelProvider::from_config)
        .unwrap_or_default();

    let tunnel_ngrok = root.get("tunnel").and_then(|t| t.get("ngrok"));
    let ngrok_auth_token = tunnel_ngrok
        .and_then(|n| n.get("auth_token"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let ngrok_domain = tunnel_ngrok
        .and_then(|n| n.get("domain"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let tunnel_cloudflare = root.get("tunnel").and_then(|t| t.get("cloudflare"));
    let cloudflare_tunnel_token = tunnel_cloudflare
        .and_then(|c| c.get("tunnel_token"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let cloudflare_hostname = tunnel_cloudflare
        .and_then(|c| c.get("hostname"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let raw_channels = root
        .get("channels")
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    // --- Workspaces (new format) with backward compat for old working_dir ---
    let workspaces: Vec<PathBuf> = root
        .get("workspaces")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| expand_home(s.trim()))
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default();

    let default_workspace: Option<PathBuf> = root
        .get("default_workspace")
        .and_then(|v| v.as_str())
        .map(|s| expand_home(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        // Backward compat: if old "working_dir" exists and no new fields, use it
        .or_else(|| {
            if root.get("workspaces").is_none() {
                root.get("working_dir")
                    .and_then(|v| v.as_str())
                    .map(|s| expand_home(s.trim()))
                    .filter(|p| !p.as_os_str().is_empty())
            } else {
                None
            }
        });

    let preview_base_url = root
        .get("preview_base_url")
        .or_else(|| root.get("tunnel").and_then(|t| t.get("preview_base_url")))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let tmux_detach_others = root
        .get("tmux")
        .and_then(|t| t.get("detach_others"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let default_agent = root
        .get("default_agent")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "claude".to_string());

    let enabled_agents = root
        .get("enabled_agents")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(crate::agent_factory::agents::AgentKind::from_str_loose)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| crate::agent_factory::agents::AgentKind::all().to_vec());

    Config {
        tunnel_provider,
        ngrok_auth_token,
        ngrok_domain,
        cloudflare_tunnel_token,
        cloudflare_hostname,
        workspaces,
        default_workspace,
        preview_base_url,
        tmux_detach_others,
        default_agent,
        enabled_agents,
        raw_channels,
    }
}

/// Base URL for preview links. Reads from the config cache.
pub fn preview_base_url() -> Option<String> {
    let cfg = ensure_loaded();
    cfg.preview_base_url.clone()
        .filter(|s| !s.is_empty())
        .or_else(|| cfg.cloudflare_hostname.as_ref().map(|h| format!("https://{}", h.trim())))
        .or_else(|| cfg.ngrok_domain.as_ref().map(|d| format!("https://{}", d.trim())))
}

/// Parse verbose config from a channel JSON object.
fn parse_verbose_config(channel_obj: Option<&serde_json::Value>) -> ImVerboseConfig {
    let verbose = channel_obj.and_then(|c| c.get("verbose"));
    ImVerboseConfig {
        show_thinking: verbose
            .and_then(|v| v.get("show_thinking"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        show_tool_use: verbose
            .and_then(|v| v.get("show_tool_use"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

/// Expand ~ to home directory in a path string.
fn expand_home(s: &str) -> PathBuf {
    if s.starts_with("~/") || s == "~" {
        home_dir().join(&s[2..])
    } else {
        PathBuf::from(s)
    }
}

/// The built-in workspaces root: ~/.vibearound/workspaces/
pub fn builtin_workspaces_dir() -> PathBuf {
    data_dir().join("workspaces")
}

/// Read + write settings.json atomically (for API-driven updates).
/// Automatically reloads the in-memory config cache after writing.
pub fn update_settings_json(mutator: impl FnOnce(&mut serde_json::Value)) -> Result<(), String> {
    let path = data_dir().join("settings.json");
    let data = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value = serde_json::from_str(&data).unwrap_or(serde_json::json!({}));
    mutator(&mut root);
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(&path, pretty).map_err(|e| e.to_string())?;
    // Invalidate cache so next ensure_loaded() picks up the change.
    *CONFIG_CACHE.write() = None;
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tunnel_provider: TunnelProvider::default(),
            ngrok_auth_token: None,
            ngrok_domain: None,
            cloudflare_tunnel_token: None,
            cloudflare_hostname: None,
            workspaces: vec![],
            default_workspace: None,
            preview_base_url: None,
            tmux_detach_others: true,
            default_agent: "claude".to_string(),
            enabled_agents: crate::agent_factory::agents::AgentKind::all().to_vec(),
            raw_channels: serde_json::Value::Object(serde_json::Map::new()),
        }
    }
}
