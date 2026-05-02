//! `handle_prompt` — the main prompt-processing path.
//!
//! Parses slash commands, short-circuits for built-in actions, or
//! forwards the content blocks to `ConversationManager::prompt` wrapped in a
//! `ChannelBridgeHandler`.

use std::sync::Arc;

use agent_client_protocol as acp;

use crate::routing::RouteKey;
use crate::conversations::ConversationManager;
use crate::agent::AgentClientHandler;

use crate::channels::bridge_handler::ChannelBridgeHandler;
use crate::channels::plugin_host::PluginHost;
use crate::channels::slash::{parse_slash_command, SlashAction};
use crate::channels::types::ChannelOutput;

use super::handover::handle_handover;
use super::mode::{handle_set_mode, set_session_mode_and_reply};
use super::send_system_text;

/// Handle a prompt request: process slash commands, then call through to
/// `ConversationManager::prompt`. Returns the real `PromptResponse` with actual
/// `StopReason`.
///
/// Used by both the channel-input processing loop (web) and the stdio
/// plugin transport (where `prompt()` blocks until the turn completes).
pub(crate) async fn handle_prompt(
    conversation_manager: &Arc<ConversationManager>,
    plugin_host: &Arc<PluginHost>,
    route: RouteKey,
    cli_kind: Option<String>,
    mut content_blocks: Vec<acp::ContentBlock>,
) -> acp::Result<acp::PromptResponse> {
    // Extract text from first Text block for slash command parsing
    let text = content_blocks
        .iter()
        .find_map(|b| match b {
            acp::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .unwrap_or_default();

    if let Some(action) = parse_slash_command(&text) {
        match action {
            SlashAction::AgentPassthrough(agent_text) => {
                // Replace text in the first Text block, or insert one
                let replaced = content_blocks.iter_mut().any(|b| {
                    if let acp::ContentBlock::Text(t) = b {
                        *t = acp::TextContent::new(&agent_text);
                        true
                    } else {
                        false
                    }
                });
                if !replaced {
                    content_blocks.insert(
                        0,
                        acp::ContentBlock::Text(acp::TextContent::new(agent_text)),
                    );
                }
            }
            SlashAction::NewSession => {
                conversation_manager.reset_session(&route).await;
                send_system_text(plugin_host, &route, "Session reset.").await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::SwitchAgent(kind) => {
                match conversation_manager.switch_agent(&route, kind.clone()).await {
                    Ok(agent_id) => {
                        send_system_text(
                            plugin_host,
                            &route,
                            &format!("Switched to {}.", agent_id),
                        )
                        .await;
                    }
                    Err(e) => {
                        send_system_text(plugin_host, &route, &format!("❌ {}", e)).await;
                    }
                }
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::SwitchProfile(profile) => {
                conversation_manager.switch_profile(&route, profile.clone()).await;
                send_system_text(
                    plugin_host,
                    &route,
                    &format!("Switched to profile {}.", profile),
                )
                .await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Close => {
                conversation_manager.close(&route, Some("user closed".to_string())).await;
                send_system_text(plugin_host, &route, "Conversation closed.").await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::ShowCommandMenu => {
                let system_commands = serde_json::to_value(&crate::resources::COMMANDS.system_commands)
                    .unwrap_or(serde_json::json!([]));
                plugin_host
                    .send_output(ChannelOutput::CommandMenu {
                        route: route.clone(),
                        system_commands,
                        agent_commands: serde_json::json!([]),
                    })
                    .await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::ListAgentCommands => {
                let agent_commands = conversation_manager.list_agent_commands(&route).await;
                plugin_host
                    .send_output(ChannelOutput::CommandMenu {
                        route: route.clone(),
                        system_commands: serde_json::json!([]),
                        agent_commands,
                    })
                    .await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::PickupCode(code) => {
                match crate::conversations::handover::pickup_codes::consume(&code) {
                    Some((agent_kind, session_id, cwd)) => {
                        match conversation_manager
                            .prepare_pickup(
                                route.clone(),
                                agent_kind.clone(),
                                session_id.clone(),
                                Some(cwd),
                            )
                            .await
                        {
                            Ok(()) => {
                                send_system_text(
                                    plugin_host,
                                    &route,
                                    &format!(
                                        "Session pickup ready (agent={}, session={}).\nSend your next message to continue.",
                                        agent_kind, session_id
                                    ),
                                )
                                .await;
                            }
                            Err(e) => {
                                send_system_text(plugin_host, &route, &format!("❌ {}", e)).await;
                            }
                        }
                    }
                    None => {
                        send_system_text(
                            plugin_host,
                            &route,
                            "❌ Invalid or expired pickup code. Please re-run the handover to get a new code.",
                        )
                        .await;
                    }
                }
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Pickup { agent_kind, session_id, cwd } => {
                match conversation_manager
                    .prepare_pickup(
                        route.clone(),
                        agent_kind.clone(),
                        session_id.clone(),
                        cwd.clone(),
                    )
                    .await
                {
                    Ok(()) => {
                        send_system_text(
                            plugin_host,
                            &route,
                            &format!(
                                "Session pickup ready (agent={}, session={}).\nSend your next message to continue.",
                                agent_kind, session_id
                            ),
                        )
                        .await;
                    }
                    Err(e) => {
                        send_system_text(plugin_host, &route, &format!("❌ {}", e)).await;
                    }
                }
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Pair(code) => {
                match crate::auth::pair::validate(&code) {
                    Some(_token) => {
                        send_system_text(
                            plugin_host,
                            &route,
                            "✅ Browser paired successfully. You can now access the dashboard.",
                        )
                        .await;
                    }
                    None => {
                        send_system_text(
                            plugin_host,
                            &route,
                            "❌ Invalid or expired pairing code. Please refresh the dashboard page and try again.",
                        )
                        .await;
                    }
                }
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Handover => {
                handle_handover(conversation_manager, plugin_host, &route).await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::PlanMode => {
                set_session_mode_and_reply(conversation_manager, plugin_host, &route, "plan").await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::SetMode(mode_id) => {
                handle_set_mode(conversation_manager, plugin_host, &route, &mode_id).await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Unknown(cmd) => {
                send_system_text(plugin_host, &route, &format!("Unknown command: {}", cmd)).await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
        }
    }

    if content_blocks.is_empty() {
        return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
    }

    tracing::debug!(
        route = %route,
        cli_kind = ?cli_kind,
        blocks = content_blocks.len(),
        "forwarding prompt to ConversationManager"
    );

    let handler: Arc<dyn AgentClientHandler> = Arc::new(ChannelBridgeHandler::new(
        Arc::clone(plugin_host),
        Arc::clone(conversation_manager),
        route.clone(),
    ));

    conversation_manager.prompt(route, cli_kind, content_blocks, handler).await
}
