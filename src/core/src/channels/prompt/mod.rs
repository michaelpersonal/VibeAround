//! Channel-input dispatch.
//!
//! `handle_channel_input` is the single entry point for every inbound
//! `ChannelInput` from stdio plugins or the web chat. It routes by
//! variant:
//!
//! - `Message` / `Callback` → [`handler::handle_prompt`] (slash
//!   command parse → ConversationManager prompt).
//! - `Stop` / `Close` / `SwitchAgent` → direct `ConversationManager` calls.
//! - `Log` → forward to the daemon log stream.
//!
//! Sub-modules:
//! - [`handler`]   — `handle_prompt` + slash-command dispatch.
//! - [`handover`]  — Direction-2 session export (generate resume command).
//! - [`mode`]      — `/mode <id>` handling.

mod handler;
mod handover;
mod mode;

use std::sync::Arc;

use agent_client_protocol as acp;

use crate::routing::RouteKey;
use crate::conversations::ConversationManager;

use super::plugin_host::PluginHost;
use super::types::{ChannelInput, ChannelOutput};

pub(crate) use handler::handle_prompt;

/// Dispatch a single `ChannelInput` to the right subsystem. Used by both the
/// stdio plugin transport and the legacy web-chat channel-input thread.
pub async fn handle_channel_input(
    conversation_manager: &Arc<ConversationManager>,
    plugin_host: &Arc<PluginHost>,
    input: ChannelInput,
) {
    match input {
        ChannelInput::Message { envelope }
        | ChannelInput::Callback {
            envelope,
            action_value: _,
        } => {
            let route = envelope.route.clone();
            let cli_kind = envelope.cli_kind.clone();
            let text = envelope.text.clone();
            tracing::debug!(
                route = %route,
                cli_kind = ?cli_kind,
                text = %text,
                "channel input"
            );

            // Wrap text into content blocks for backward compat (web chat path)
            let content_blocks = if text.is_empty() {
                vec![]
            } else {
                vec![acp::ContentBlock::Text(acp::TextContent::new(text))]
            };

            match handle_prompt(conversation_manager, plugin_host, route.clone(), cli_kind, content_blocks)
                .await
            {
                Ok(_resp) => {
                    tracing::debug!(route = %route, "prompt ok");
                }
                Err(e) => {
                    tracing::warn!(route = %route, error = %e, "prompt failed");
                    send_system_text(plugin_host, &route, &format!("❌ {}", e)).await;
                }
            }
        }
        ChannelInput::Stop { route } => {
            let _ = conversation_manager.cancel(&route).await;
        }
        ChannelInput::Close { route, reason } => {
            conversation_manager.close(&route, reason).await;
        }
        ChannelInput::SwitchAgent { route, agent_kind } => {
            if let Err(e) = conversation_manager.switch_agent(&route, agent_kind).await {
                tracing::warn!(route = %route, error = %e, "switch agent failed");
                send_system_text(plugin_host, &route, &format!("❌ {}", e)).await;
            }
        }
        ChannelInput::Log { level, message } => {
            tracing::info!(
                level = %level.unwrap_or_else(|| "info".to_string()),
                message = %message,
                "channel log"
            );
        }
    }
}

/// Fire-and-forget helper: emit a `SystemText` to the plugin for this route.
/// Shared by every sub-module in this folder.
pub(super) async fn send_system_text(
    plugin_host: &Arc<PluginHost>,
    route: &RouteKey,
    text: &str,
) {
    plugin_host
        .send_output(ChannelOutput::SystemText {
            route: route.clone(),
            text: text.to_string(),
            reply_to: None,
        })
        .await;
}
