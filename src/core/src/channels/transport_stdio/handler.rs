//! `PluginAgentHandler` — the host's `acp::Agent` implementation that the
//! plugin connects to. Drives the prompt lifecycle and routes
//! `ext_notification` messages back into the host.

use std::sync::Arc;

use tokio::sync::mpsc;

use agent_client_protocol as acp;

use crate::routing::RouteKey;
use crate::conversations::ConversationManager;

use super::super::plugin_host::PluginHost;
use super::super::prompt::handle_prompt;
use super::super::{ChannelEnvelope, ChannelInput, ChannelOutput};

/// ACP Agent handler for a channel plugin. `prompt()` calls through to
/// `handle_prompt()` directly — blocks until the turn completes and
/// returns the real `PromptResponse` with `StopReason`.
pub(super) struct PluginAgentHandler {
    channel_kind: String,
    config: serde_json::Value,
    /// Still used for fire-and-forget operations: cancel, callback.
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    conversation_manager: Arc<ConversationManager>,
    plugin_host: Arc<PluginHost>,
}

impl PluginAgentHandler {
    pub(super) fn new(
        channel_kind: String,
        config: serde_json::Value,
        input_tx: mpsc::UnboundedSender<ChannelInput>,
        conversation_manager: Arc<ConversationManager>,
        plugin_host: Arc<PluginHost>,
    ) -> Self {
        Self {
            channel_kind,
            config,
            input_tx,
            conversation_manager,
            plugin_host,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for PluginAgentHandler {
    async fn initialize(
        &self,
        _args: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        tracing::info!("[{}] ACP initialize from plugin", self.channel_kind);

        let mut meta = serde_json::Map::new();
        meta.insert("channelKind".into(), self.channel_kind.clone().into());
        meta.insert("config".into(), self.config.clone());
        meta.insert("hostVersion".into(), env!("CARGO_PKG_VERSION").into());
        meta.insert(
            "cacheDir".into(),
            crate::config::data_dir()
                .join(".cache")
                .to_string_lossy()
                .into(),
        );

        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_info(
                acp::Implementation::new("vibearound-host", env!("CARGO_PKG_VERSION"))
                    .title("VibeAround"),
            )
            .meta(meta))
    }

    async fn authenticate(
        &self,
        _args: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        Err(acp::Error::method_not_found())
    }

    async fn new_session(
        &self,
        _args: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        Err(acp::Error::method_not_found())
    }

    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        let chat_id = args.session_id.to_string();
        let route = RouteKey::new(&self.channel_kind, &chat_id);

        let content_blocks = args.prompt;

        if content_blocks.is_empty() {
            return Err(acp::Error::invalid_params());
        }

        // Extract text preview for logging
        let text_preview: String = content_blocks
            .iter()
            .find_map(|b| match b {
                acp::ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .unwrap_or_default();

        tracing::info!(
            "[{}] ACP prompt chat_id={} blocks={} text_preview={}",
            self.channel_kind,
            chat_id,
            content_blocks.len(),
            text_preview.chars().take(80).collect::<String>()
        );

        // Call through to handle_prompt — blocks until the turn completes.
        // Session notifications stream to the plugin via ChannelBridgeHandler
        // → PluginHost → output_tx → output forwarder → conn.session_notification().
        let result = handle_prompt(
            &self.conversation_manager,
            &self.plugin_host,
            route.clone(),
            None, // cli_kind: plugin prompts don't specify
            content_blocks,
        )
        .await;

        // Surface detailed error to the IM chat so the user sees more than
        // the plugin's default "Internal error" rendering. The ACP Error's
        // Display impl includes `data.details` when present.
        if let Err(ref e) = result {
            self.plugin_host
                .send_output(ChannelOutput::SystemText {
                    route,
                    text: format!("⚠️ {}", e),
                    reply_to: None,
                })
                .await;
        }

        result
    }

    async fn cancel(&self, args: acp::CancelNotification) -> acp::Result<()> {
        let chat_id = args.session_id.to_string();
        let route = RouteKey::new(&self.channel_kind, &chat_id);

        tracing::info!("[{}] ACP cancel chat_id={}", self.channel_kind, chat_id);

        let _ = self.input_tx.send(ChannelInput::Stop { route });
        Ok(())
    }

    async fn ext_notification(&self, args: acp::ExtNotification) -> acp::Result<()> {
        // Rust ACP SDK already strips the "_" prefix before dispatching here.
        let method = args.method.to_string();
        let params: serde_json::Value = serde_json::from_str(args.params.get())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        let params_obj = params.as_object().cloned().unwrap_or_default();

        match method.as_str() {
            "va/heartbeat" => {
                super::super::monitor::touch_weak(
                    &self.plugin_host.monitor_weak(),
                    &self.channel_kind,
                );
            }
            "va/callback" => {
                // Accept both chatId (new) and channelId (legacy, "kind:chatId") for compat.
                let chat_id = params_obj
                    .get("chatId")
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        params_obj
                            .get("channelId")
                            .and_then(|v| v.as_str())
                            .map(|cid| {
                                cid.strip_prefix(&format!("{}:", self.channel_kind))
                                    .unwrap_or(cid)
                            })
                    })
                    .unwrap_or("");
                let route = RouteKey::new(&self.channel_kind, chat_id);
                let action_value = params_obj
                    .get("data")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let input = ChannelInput::Callback {
                    envelope: ChannelEnvelope {
                        route,
                        message_id: params_obj
                            .get("messageId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        turn_id: None,
                        text: String::new(),
                        sender_id: params_obj
                            .get("sender")
                            .and_then(|v| v.get("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        attachments: vec![],
                        parent_id: None,
                        cli_kind: None,
                    },
                    action_value,
                };
                let _ = self.input_tx.send(input);
            }
            other => {
                tracing::info!(
                    "[{}] unhandled ext_notification: {}",
                    self.channel_kind, other
                );
            }
        }
        Ok(())
    }

    async fn ext_method(&self, args: acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let method = args.method.to_string();
        tracing::info!("[{}] unhandled ext_method: {}", self.channel_kind, method);
        Err(acp::Error::method_not_found())
    }
}
