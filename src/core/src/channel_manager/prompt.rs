//! Channel-input dispatch + slash-command execution.
//!
//! `handle_channel_input` is the single entry point for every inbound
//! `ChannelInput` message (from stdio plugins or the web chat). For
//! `Message` / `Callback` variants it falls through to `handle_prompt`,
//! which consults the slash parser and either short-circuits with a
//! system-text reply or forwards the content blocks to `ACPHub::prompt`
//! behind a fresh `ChannelBridgeHandler`.

use std::sync::Arc;

use agent_client_protocol as acp;

use crate::acp::routing::RouteKey;
use crate::acp_hub::ACPHub;
use crate::agent_factory::runtime::BridgeClientHandler;

use super::bridge_handler::ChannelBridgeHandler;
use super::plugin_host::PluginHost;
use super::slash::{parse_slash_command, SlashAction};
use super::types::{ChannelInput, ChannelOutput};

/// Dispatch a single `ChannelInput` to the right subsystem. Used by both the
/// stdio plugin transport and the legacy web-chat channel-input thread.
pub async fn handle_channel_input(
    acp_hub: &Arc<ACPHub>,
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
            eprintln!(
                "[ChannelManager] input route={} cli_kind={:?} text={:?}",
                route, cli_kind, text
            );

            // Wrap text into content blocks for backward compat (web chat path)
            let content_blocks = if text.is_empty() {
                vec![]
            } else {
                vec![acp::ContentBlock::Text(acp::TextContent::new(text))]
            };

            match handle_prompt(acp_hub, plugin_host, route.clone(), cli_kind, content_blocks)
                .await
            {
                Ok(_resp) => {
                    eprintln!("[ChannelManager] prompt OK route={}", route);
                }
                Err(e) => {
                    eprintln!("[ChannelManager] prompt ERR route={} error={}", route, e);
                    send_system_text(plugin_host, &route, &format!("❌ {}", e)).await;
                }
            }
        }
        ChannelInput::Stop { route } => {
            let _ = acp_hub.cancel(&route).await;
        }
        ChannelInput::Close { route, reason } => {
            acp_hub.close(&route, reason).await;
        }
        ChannelInput::SwitchAgent { route, agent_kind } => {
            acp_hub.switch_agent(&route, agent_kind).await;
        }
        ChannelInput::Log { level, message } => {
            eprintln!(
                "[ChannelManager][channel][{}] {}",
                level.unwrap_or_else(|| "info".to_string()),
                message
            );
        }
    }
}

/// Handle a prompt request: process slash commands, then call through to
/// `ACPHub::prompt`. Returns the real `PromptResponse` with actual
/// `StopReason`.
///
/// Used by both the channel-input processing loop (web) and the stdio plugin
/// transport (where `prompt()` blocks until the turn completes).
pub(crate) async fn handle_prompt(
    acp_hub: &Arc<ACPHub>,
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

    // Check for slash commands
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
                acp_hub.reset_session(&route).await;
                send_system_text(plugin_host, &route, "Session reset.").await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::SwitchAgent(kind) => {
                acp_hub.switch_agent(&route, kind.clone()).await;
                send_system_text(plugin_host, &route, &format!("Switched to {}.", kind)).await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::SwitchProfile(profile) => {
                acp_hub.switch_profile(&route, profile.clone()).await;
                send_system_text(
                    plugin_host,
                    &route,
                    &format!("Switched to profile {}.", profile),
                )
                .await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Close => {
                acp_hub.close(&route, Some("user closed".to_string())).await;
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
                let agent_commands = acp_hub.list_agent_commands(&route).await;
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
                match crate::pickup_codes::consume(&code) {
                    Some((agent_kind, session_id, cwd)) => {
                        acp_hub
                            .prepare_pickup(
                                route.clone(),
                                agent_kind.clone(),
                                session_id.clone(),
                                Some(cwd),
                            )
                            .await;
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
                acp_hub
                    .prepare_pickup(
                        route.clone(),
                        agent_kind.clone(),
                        session_id.clone(),
                        cwd.clone(),
                    )
                    .await;
                send_system_text(
                    plugin_host,
                    &route,
                    &format!(
                        "Session pickup ready (agent={}, session={}).\nSend your next message to continue.",
                        agent_kind, session_id
                    ),
                )
                .await;
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
                handle_handover(acp_hub, plugin_host, &route).await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::PlanMode => {
                set_session_mode_and_reply(acp_hub, plugin_host, &route, "plan").await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::SetMode(mode_id) => {
                handle_set_mode(acp_hub, plugin_host, &route, &mode_id).await;
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

    eprintln!(
        "[ChannelManager] prompt route={} cli_kind={:?} blocks={}",
        route,
        cli_kind,
        content_blocks.len()
    );

    let handler: Arc<dyn BridgeClientHandler> = Arc::new(ChannelBridgeHandler::new(
        Arc::clone(plugin_host),
        Arc::clone(acp_hub),
        route.clone(),
    ));

    acp_hub.prompt(route, cli_kind, content_blocks, handler).await
}

/// Direction 2 handover: export the current session to a coding agent CLI.
/// Sends the user a resume command they can paste into their terminal.
async fn handle_handover(
    acp_hub: &Arc<ACPHub>,
    plugin_host: &Arc<PluginHost>,
    route: &RouteKey,
) {
    let snapshot = acp_hub.snapshot(route).await;
    match snapshot {
        Some(snap) if snap.session_id.is_some() => {
            let session_id = snap.session_id.unwrap();
            let cwd = snap.workspace.unwrap_or_else(|| "~".to_string());
            let cli_kind = snap.cli_kind.unwrap_or_else(|| "claude".to_string());
            let resume_cmd = crate::resources::agent_by_id(&cli_kind)
                .and_then(|a| a.resume_template.as_ref())
                .map(|tpl| tpl.replace("{cwd}", &cwd).replace("{session_id}", &session_id))
                .unwrap_or_else(|| {
                    format!("cd {} && {} (resume session {})", cwd, cli_kind, session_id)
                });
            send_system_text(
                plugin_host,
                route,
                &format!(
                    "Run this in your terminal to continue the session:\n\n{}\n\nYou can close this chat after resuming.",
                    resume_cmd
                ),
            )
            .await;
        }
        _ => {
            send_system_text(
                plugin_host,
                route,
                "No active session to hand over. Send a message first to start a session.",
            )
            .await;
        }
    }
}

/// Validate + canonicalise the mode ID from `/mode <id>`, then dispatch.
async fn handle_set_mode(
    acp_hub: &Arc<ACPHub>,
    plugin_host: &Arc<PluginHost>,
    route: &RouteKey,
    mode_id: &str,
) {
    const VALID: &[&str] = &[
        "default",
        "plan",
        "acceptEdits",
        "bypassPermissions",
        "dontAsk",
    ];
    let canonical = match mode_id {
        "accept_edits" | "accept-edits" | "accept" => "acceptEdits",
        "bypass_permissions" | "bypass-permissions" | "bypass" => "bypassPermissions",
        "dont_ask" | "dont-ask" | "dontask" => "dontAsk",
        other => other,
    };
    if !VALID.contains(&canonical) {
        send_system_text(
            plugin_host,
            route,
            &format!("Unknown mode `{}`. Valid: {}.", mode_id, VALID.join(", ")),
        )
        .await;
    } else {
        set_session_mode_and_reply(acp_hub, plugin_host, route, canonical).await;
    }
}

/// Fire-and-forget helper: emit a `SystemText` to the plugin for this route.
pub(crate) async fn send_system_text(
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

/// Call `set_session_mode` on the current pod and report the outcome via
/// system text. Relies on the agent to emit `current_mode_update` which the
/// plugin SDK renders as a mode badge — we only send a confirmation line
/// here so the user sees their command was accepted even before the agent
/// replies.
async fn set_session_mode_and_reply(
    acp_hub: &Arc<ACPHub>,
    plugin_host: &Arc<PluginHost>,
    route: &RouteKey,
    mode_id: &str,
) {
    match acp_hub.set_session_mode(route, mode_id.to_string()).await {
        Ok(()) => {
            send_system_text(
                plugin_host,
                route,
                &format!("✅ Mode switched to `{}`.", mode_id),
            )
            .await;
        }
        Err(error) => {
            send_system_text(
                plugin_host,
                route,
                &format!(
                    "❌ Could not switch mode to `{}`: {}. Start a conversation first, then try `/mode {}`.",
                    mode_id, error, mode_id,
                ),
            )
            .await;
        }
    }
}
