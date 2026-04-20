//! VibeAround core: ACP hub, agent factory, channels, PTY, tunnels, workspace. No UI, no HTTP.

pub mod acp;
pub mod acp_hub;
pub mod agent_factory;
pub mod agent_integrations;
pub mod auth;
pub mod channel_manager;
pub mod child_registry;
pub mod config;
pub mod env;
pub mod logging;
pub mod pickup_codes;
pub mod preview_entries;
pub mod plugins;
pub mod pty;
pub mod resources;
pub mod state;
pub mod tunnels;
pub mod workspace;
