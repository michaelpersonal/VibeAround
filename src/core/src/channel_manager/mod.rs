//! ACP-native channel manager: hosts channel plugins and routes traffic.
//!
//! The web channel path uses ACP directly (ws_chat dispatches via ACPHub).
//! Stdio plugins still use the legacy ChannelInput/ChannelOutput for now.

pub mod manifest;
pub mod plugin_host;
pub mod plugin_runtime;
pub mod transport_stdio;
pub mod transport_websocket;

use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use tokio::sync::broadcast;

use crate::acp::routing::{Attachment, MessageId, RouteEnvelope, RouteKey, TurnId};
use crate::acp_hub::event::SystemEvent;
use crate::acp_hub::ACPHub;
use crate::agent_factory::runtime::BridgeClientHandler;
use crate::plugins::DiscoveredPlugin;

use agent_client_protocol as acp;

use self::manifest::ChannelPluginManifest;
use self::plugin_host::PluginHost;

pub use self::transport_websocket::WebChannelManager;

/// Legacy envelope kept for stdio plugin compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelEnvelope {
    pub route: RouteKey,
    #[serde(default)]
    pub message_id: MessageId,
    #[serde(default)]
    pub turn_id: Option<TurnId>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub sender_id: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub cli_kind: Option<String>,
}

impl ChannelEnvelope {
    pub fn into_route_envelope(self) -> RouteEnvelope {
        RouteEnvelope {
            channel_kind: self.route.channel_kind,
            chat_id: self.route.chat_id,
            message_id: self.message_id,
            turn_id: self.turn_id,
            text: self.text,
            sender_id: self.sender_id,
            attachments: self.attachments,
            parent_id: self.parent_id,
            cli_kind: self.cli_kind,
        }
    }
}

/// Legacy stdio plugin input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ChannelInput {
    Message {
        #[serde(flatten)]
        envelope: ChannelEnvelope,
    },
    Callback {
        #[serde(flatten)]
        envelope: ChannelEnvelope,
        #[serde(default)]
        action_value: Option<String>,
    },
    Stop {
        route: RouteKey,
    },
    Close {
        route: RouteKey,
        #[serde(default)]
        reason: Option<String>,
    },
    SwitchAgent {
        route: RouteKey,
        agent_kind: String,
    },
    Log {
        #[serde(default)]
        level: Option<String>,
        message: String,
    },
}

/// Channel plugin output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ChannelOutput {
    RawAcp {
        route: RouteKey,
        payload: serde_json::Value,
    },
    SystemText {
        route: RouteKey,
        text: String,
        reply_to: Option<MessageId>,
    },
    AgentReady {
        route: RouteKey,
        agent: String,
        version: String,
    },
    SessionReady {
        route: RouteKey,
        session_id: String,
    },
}

impl ChannelOutput {
    pub fn route_key(&self) -> &RouteKey {
        match self {
            Self::RawAcp { route, .. }
            | Self::SystemText { route, .. }
            | Self::AgentReady { route, .. }
            | Self::SessionReady { route, .. } => route,
        }
    }
}

pub struct ChannelManager {
    plugin_host: Arc<PluginHost>,
    /// Channel for fire-and-forget input dispatch.
    /// `handle_input` sends here; the processing loop runs on a dedicated
    /// `spawn_local` task so that `!Send` ACP futures are allowed.
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    input_rx: StdMutex<Option<mpsc::UnboundedReceiver<ChannelInput>>>,
    acp_hub: Arc<ACPHub>,
}

impl ChannelManager {
    pub fn new(acp_hub: Arc<ACPHub>) -> Self {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        Self {
            plugin_host: Arc::new(PluginHost::new(input_tx.clone())),
            input_tx,
            input_rx: StdMutex::new(Some(input_rx)),
            acp_hub,
        }
    }

    pub fn plugin_host(&self) -> Arc<PluginHost> {
        Arc::clone(&self.plugin_host)
    }

    /// Take the input receiver so the caller can drive the processing loop.
    /// This must be called exactly once (typically during daemon startup).
    pub fn take_input_rx(&self) -> Option<mpsc::UnboundedReceiver<ChannelInput>> {
        self.input_rx.lock().unwrap().take()
    }

    pub async fn start_plugin(
        &self,
        channel_name: &str,
        plugin: &DiscoveredPlugin,
    ) -> Option<AbortHandle> {
        let manifest = match ChannelPluginManifest::from_discovered(channel_name.to_string(), plugin) {
            Some(manifest) => manifest,
            None => {
                eprintln!(
                    "[{}] config=missing channels.{} — plugin disabled",
                    channel_name, channel_name
                );
                return None;
            }
        };

        match self
            .plugin_host
            .register_stdio_plugin(
                manifest,
                Arc::clone(&self.acp_hub),
                Arc::clone(&self.plugin_host),
            )
            .await
        {
            Ok(abort_handle) => Some(abort_handle),
            Err(error) => {
                eprintln!("[{}] failed to start plugin: {}", channel_name, error);
                None
            }
        }
    }

    pub fn start_internal_plugin(
        &self,
        channel_name: &str,
        outbound_tx: mpsc::UnboundedSender<ChannelOutput>,
    ) {
        self.plugin_host
            .register_websocket_plugin(channel_name.to_string(), outbound_tx);
        eprintln!("[{}] registered internal ACP plugin", channel_name);
    }

    /// Fire-and-forget: enqueue input for async processing.
    /// This is `Send`-safe because it only does a channel send.
    pub fn handle_input(&self, input: ChannelInput) {
        let _ = self.input_tx.send(input);
    }

    /// Process a single input on the current executor.
    /// This may await `!Send` ACP futures, so callers should run it on a
    /// `LocalSet` or other non-`Send`-compatible context when needed.
    pub async fn process_input(&self, input: ChannelInput) {
        handle_channel_input(&self.acp_hub, &self.plugin_host, input).await;
    }

    pub fn acp_hub(&self) -> Arc<ACPHub> {
        Arc::clone(&self.acp_hub)
    }

    pub async fn send_output(&self, output: ChannelOutput) {
        self.plugin_host.send_output(output).await;
    }

    pub async fn shutdown_all(&self) {
        self.plugin_host.shutdown_all().await;
    }

    /// Subscribe to ACPHub SystemEvents and forward relevant ones to channel plugins.
    /// Call once during daemon startup. Returns a JoinHandle for the forwarder task.
    pub fn start_event_forwarder(
        &self,
        mut event_rx: broadcast::Receiver<SystemEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let plugin_host = Arc::clone(&self.plugin_host);
        tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => forward_system_event(&plugin_host, &event).await,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }
}

async fn forward_system_event(plugin_host: &Arc<PluginHost>, event: &SystemEvent) {
    match event {
        SystemEvent::AgentInitialized {
            route,
            cli_kind,
            initialize,
            ..
        } => {
            let agent_info = initialize.agent_info.as_ref();
            let agent = agent_info
                .map(|i| i.title.clone().unwrap_or_else(|| i.name.clone()))
                .or_else(|| cli_kind.clone())
                .unwrap_or_else(|| "agent".to_string());
            let version = agent_info
                .map(|i| i.version.clone())
                .unwrap_or_default();
            plugin_host
                .send_output(ChannelOutput::AgentReady {
                    route: route.clone(),
                    agent,
                    version,
                })
                .await;
        }
        SystemEvent::SessionReady {
            route, session_id,
        } => {
            plugin_host
                .send_output(ChannelOutput::SessionReady {
                    route: route.clone(),
                    session_id: session_id.clone(),
                })
                .await;
        }
        _ => {}
    }
}

/// Parse system slash commands from prompt text.
/// Returns None if the text is not a slash command (regular prompt).
enum SlashAction {
    /// /agent <rest> — strip prefix, send rest as prompt to agent CLI
    AgentPassthrough(String),
    /// /new — reset session (new conversation, same agent)
    NewSession,
    /// /switch <agent_kind> — switch agent
    SwitchAgent(String),
    /// /profile <profile> — switch profile
    SwitchProfile(String),
    /// /close — close route
    Close,
    /// /help or /commands — list available agent commands
    ListAgentCommands,
    /// /pickup <session_id> <cwd> [agent_kind] — import a session from a coding agent (Direction 1)
    Pickup { session_id: String, cwd: String, agent_kind: Option<String> },
    /// /handover — export current session to a coding agent (Direction 2)
    Handover,
    /// Unknown slash command
    Unknown(String),
}

fn parse_slash_command(text: &str) -> Option<SlashAction> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    // /agent <rest> — passthrough to agent CLI as a slash command
    if let Some(rest) = trimmed.strip_prefix("/agent/") {
        let rest = rest.trim();
        if !rest.is_empty() {
            return Some(SlashAction::AgentPassthrough(format!("/{}", rest)));
        }
    }
    if let Some(rest) = trimmed.strip_prefix("/agent ") {
        let rest = rest.trim();
        if !rest.is_empty() {
            let cmd = if rest.starts_with('/') {
                rest.to_string()
            } else {
                format!("/{}", rest)
            };
            return Some(SlashAction::AgentPassthrough(cmd));
        }
    }
    if trimmed == "/agent" {
        return Some(SlashAction::AgentPassthrough("/agent".to_string()));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim().to_string());

    match cmd {
        "/new" => Some(SlashAction::NewSession),
        "/switch" => match arg {
            Some(kind) if !kind.is_empty() => Some(SlashAction::SwitchAgent(kind)),
            _ => Some(SlashAction::Unknown(trimmed.to_string())),
        },
        "/profile" => match arg {
            Some(profile) if !profile.is_empty() => Some(SlashAction::SwitchProfile(profile)),
            _ => Some(SlashAction::Unknown(trimmed.to_string())),
        },
        "/close" => Some(SlashAction::Close),
        "/help" | "/commands" | "/list_agent_commands" => Some(SlashAction::ListAgentCommands),
        "/pickup" => {
            // /pickup <session_id> <cwd> [agent_kind]
            // session_id is the first token, then cwd, then optional agent_kind
            match arg {
                Some(rest) if !rest.is_empty() => {
                    let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                    if parts.len() >= 2 && !parts[1].is_empty() {
                        Some(SlashAction::Pickup {
                            session_id: parts[0].to_string(),
                            cwd: parts[1].to_string(),
                            agent_kind: parts.get(2).map(|s| s.to_string()),
                        })
                    } else {
                        Some(SlashAction::Unknown(trimmed.to_string()))
                    }
                }
                _ => Some(SlashAction::Unknown(trimmed.to_string())),
            }
        }
        "/handover" => Some(SlashAction::Handover),
        _ => Some(SlashAction::Unknown(trimmed.to_string())),
    }
}

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

/// Handle a prompt request: process slash commands, then call through to ACPHub.
/// Returns the real `PromptResponse` with the actual `StopReason`.
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
                send_system_text(plugin_host, &route, &format!("Switched to {}.", kind))
                    .await;
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
            SlashAction::ListAgentCommands => {
                let commands = acp_hub.list_agent_commands(&route).await;
                let text = format_agent_commands(&commands);
                send_system_text(plugin_host, &route, &text).await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Pickup { session_id, cwd, agent_kind } => {
                // Direction 1: Agent → IM. User sends /pickup <session_id> <cwd> [agent_kind]
                // to import a session from a coding agent.
                let cli_kind = agent_kind
                    .unwrap_or_else(|| crate::config::ensure_loaded().default_agent.clone());
                acp_hub.prepare_pickup(
                    route.clone(),
                    cli_kind.clone(),
                    session_id.clone(),
                    cwd.clone(),
                ).await;
                send_system_text(
                    plugin_host,
                    &route,
                    &format!(
                        "Session pickup ready (session={}, workspace={}).\nSend your next message to continue.",
                        session_id, cwd
                    ),
                )
                .await;
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Handover => {
                // Direction 2: IM → Agent. User sends /handover to export
                // current session to a coding agent CLI.
                let snapshot = acp_hub.snapshot(&route).await;
                match snapshot {
                    Some(snap) if snap.session_id.is_some() => {
                        let session_id = snap.session_id.unwrap();
                        let cwd = snap.workspace.unwrap_or_else(|| "~".to_string());
                        let cli_kind = snap.cli_kind.unwrap_or_else(|| "claude".to_string());
                        let resume_cmd = crate::resources::agent_by_id(&cli_kind)
                            .and_then(|a| a.resume_template.as_ref())
                            .map(|tpl| tpl.replace("{cwd}", &cwd).replace("{session_id}", &session_id))
                            .unwrap_or_else(|| format!("cd {} && {} (resume session {})", cwd, cli_kind, session_id));
                        send_system_text(
                            plugin_host,
                            &route,
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
                            &route,
                            "No active session to hand over. Send a message first to start a session.",
                        )
                        .await;
                    }
                }
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            SlashAction::Unknown(cmd) => {
                send_system_text(
                    plugin_host,
                    &route,
                    &format!("Unknown command: {}", cmd),
                )
                .await;
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

    let handler: Arc<dyn BridgeClientHandler> = Arc::new(ChannelBridgeHandler {
        plugin_host: Arc::clone(plugin_host),
        acp_hub: Arc::clone(acp_hub),
        route: route.clone(),
    });

    acp_hub
        .prompt(route, cli_kind, content_blocks, handler)
        .await
}

async fn send_system_text(plugin_host: &Arc<PluginHost>, route: &RouteKey, text: &str) {
    plugin_host
        .send_output(ChannelOutput::SystemText {
            route: route.clone(),
            text: text.to_string(),
            reply_to: None,
        })
        .await;
}

/// Format cached agent commands into a readable text message.
fn format_agent_commands(commands: &serde_json::Value) -> String {
    let arr = match commands.as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return "No agent commands available. Send a message first to initialize the agent.".to_string(),
    };

    let mut lines = vec!["Available agent commands (use /agent <command>):".to_string()];
    for cmd in arr {
        let name = cmd.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let desc = cmd.get("description").and_then(|v| v.as_str()).unwrap_or("");
        // Truncate long descriptions (char-safe)
        let short_desc = if desc.chars().count() > 80 {
            let truncated: String = desc.chars().take(77).collect();
            format!("{}...", truncated)
        } else {
            desc.to_string()
        };
        lines.push(format!("  /{} — {}", name, short_desc));
    }

    lines.push("\nSystem commands:".to_string());
    lines.push(crate::resources::format_system_commands_help());

    lines.join("\n")
}

struct ChannelBridgeHandler {
    plugin_host: Arc<PluginHost>,
    acp_hub: Arc<ACPHub>,
    route: RouteKey,
}

impl ChannelBridgeHandler {
    async fn send_raw_acp<T: serde::Serialize>(&self, value: &T) -> acp::Result<()> {
        let payload = serde_json::to_value(value)
            .map_err(|e| acp::Error::new(-32603, format!("serialize: {}", e)))?;
        self.plugin_host
            .send_output(ChannelOutput::RawAcp {
                route: self.route.clone(),
                payload,
            })
            .await;
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl BridgeClientHandler for ChannelBridgeHandler {
    async fn session_notification(&self, args: acp::SessionNotification) -> acp::Result<()> {
        eprintln!(
            "[ChannelBridgeHandler] session_notification route={} session={}",
            self.route, args.session_id
        );

        // Cache available_commands_update in the pod for later query
        let payload = serde_json::to_value(&args)
            .map_err(|e| acp::Error::new(-32603, format!("serialize: {}", e)))?;
        if let Some(update) = payload.get("update") {
            if update.get("sessionUpdate").and_then(|v| v.as_str()) == Some("available_commands_update") {
                if let Some(commands) = update.get("availableCommands") {
                    self.acp_hub.list_agent_commands_update(&self.route, commands.clone()).await;
                }
            }
        }

        self.plugin_host
            .send_output(ChannelOutput::RawAcp {
                route: self.route.clone(),
                payload,
            })
            .await;
        Ok(())
    }

    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        if let Some(first) = args.options.first() {
            Ok(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Selected(
                    acp::SelectedPermissionOutcome::new(first.option_id.clone()),
                ),
            ))
        } else {
            Err(acp::Error::method_not_found())
        }
    }
}
