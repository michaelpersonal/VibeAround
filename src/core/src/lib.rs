//! VibeAround core: conversation manager, agents, channels, PTY, tunnels, workspace. No UI, no HTTP.

pub mod agent;
pub mod auth;
pub mod channel_manager;
pub mod child_registry;
pub mod config;
pub mod conversation_manager;
pub mod env;
pub mod logging;
pub mod plugins;
pub mod preview_entries;
pub mod pty;
pub mod resources;
pub mod routing;
pub mod state;
pub mod tunnels;
pub mod workspace;
