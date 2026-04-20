use std::path::PathBuf;

use crate::routing::ChannelKind;
use crate::config;
use crate::plugins::DiscoveredPlugin;

#[derive(Debug, Clone)]
pub struct ChannelPluginManifest {
    pub channel_kind: ChannelKind,
    pub runtime: String,
    pub plugin_dir: PathBuf,
    pub entry_path: PathBuf,
    pub raw_config: serde_json::Value,
}

impl ChannelPluginManifest {
    pub fn from_discovered(
        channel_kind: impl Into<ChannelKind>,
        plugin: &DiscoveredPlugin,
    ) -> Option<Self> {
        let channel_kind = channel_kind.into();
        let raw_config = config::ensure_loaded().channel_raw_config(&channel_kind)?;
        Some(Self {
            channel_kind,
            runtime: plugin.manifest.runtime.clone(),
            plugin_dir: plugin.dir.clone(),
            entry_path: plugin.entry_path(),
            raw_config,
        })
    }
}
