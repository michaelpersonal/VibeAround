//! REST API handlers for the web server.
//!
//! Domain handlers live in small submodules. This facade keeps the route
//! declarations in `web_server::mod` stable while the implementation stays
//! grouped by feature area.

mod previews;
mod profiles;
mod runtime;
mod sessions;
mod workspaces;

pub use previews::{delete_preview_handler, list_previews_handler};
pub use profiles::list_profiles_handler;
pub use runtime::{
    kill_agent_handler, kill_pty_handler, kill_tunnel_handler, list_agents_handler,
    list_agents_runtime_handler, list_channels_handler, list_tunnels_handler,
    restart_channel_handler, start_channel_handler, stop_channel_handler,
};
pub use sessions::{
    create_session_handler, delete_session_handler, list_sessions_handler,
    list_tmux_sessions_handler,
};
pub use workspaces::{
    add_workspace_handler, list_workspaces_handler, remove_workspace_handler,
    set_default_workspace_handler,
};
