//! Stdio plugin transport using ACP protocol.
//!
//! The host acts as an ACP Agent toward the plugin (which acts as an ACP Client).
//! Host sends `session_notification()` to stream events back to plugin.
//!
//! ## Session ID Convention
//!
//! ACP requires a `sessionId` on `PromptRequest`. Channel plugins use the
//! **chat room identifier** (chatId) as the ACP `sessionId`. This is NOT
//! the real agent session — the host maps `(channelKind, chatId)` to an
//! internal `RouteKey` and manages the real agent session transparently.
//!
//! When forwarding `SessionNotification` back to the plugin, the host
//! **replaces** the real agent's sessionId with the chatId so the plugin
//! receives notifications matching what it sent.

use std::sync::Arc;

use serde_json::value::RawValue;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use agent_client_protocol as acp;

use super::manifest::ChannelPluginManifest;
use super::{ChannelEnvelope, ChannelInput, ChannelOutput};
use crate::acp::routing::RouteKey;

/// A running stdio plugin connected via ACP protocol.
#[derive(Debug)]
pub struct StdioPluginRuntime {
    channel_kind: String,
    /// Send ChannelOutput to the plugin via ACP session_notification.
    output_tx: mpsc::UnboundedSender<ChannelOutput>,
    abort_handle: AbortHandle,
}

impl StdioPluginRuntime {
    pub async fn spawn(
        manifest: ChannelPluginManifest,
        input_tx: mpsc::UnboundedSender<ChannelInput>,
    ) -> Result<Self, String> {
        if manifest.runtime != "node" {
            return Err(format!(
                "unsupported channel runtime '{}' for {}",
                manifest.runtime, manifest.channel_kind
            ));
        }

        if !manifest.entry_path.exists() {
            return Err(format!(
                "plugin entry not found: {}",
                manifest.entry_path.display()
            ));
        }

        let mut child = Command::new("node")
            .arg(&manifest.entry_path)
            .current_dir(&manifest.plugin_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("failed to spawn plugin process: {}", error))?;

        let stdin = child.stdin.take().ok_or("plugin stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("plugin stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("plugin stderr unavailable")?;

        let channel_kind = manifest.channel_kind.clone();

        // Stderr → log
        let stderr_channel = channel_kind.clone();
        tokio::spawn(async move {
            let reader = tokio::io::BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[{}][plugin] {}", stderr_channel, line);
            }
        });

        // Channel for outbound ChannelOutput → ACP session_notification
        let (output_tx, output_rx) = mpsc::unbounded_channel::<ChannelOutput>();

        // Spawn the ACP bridge on a dedicated thread (requires LocalSet for !Send futures)
        let acp_channel = channel_kind.clone();
        let raw_config = manifest.raw_config.clone();
        std::thread::Builder::new()
            .name(format!("{}-plugin", channel_kind))
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to build plugin runtime");
                runtime.block_on(async move {
                    let _child = child; // keep process alive
                    run_acp_plugin_bridge(
                        acp_channel,
                        raw_config,
                        stdin,
                        stdout,
                        input_tx,
                        output_rx,
                    )
                    .await;
                });
            })
            .map_err(|e| format!("Failed to spawn plugin thread: {}", e))?;

        Ok(Self {
            channel_kind,
            output_tx,
            abort_handle: tokio::task::spawn(std::future::pending::<()>()).abort_handle(), // placeholder
        })
    }

    pub fn abort_handle(&self) -> AbortHandle {
        self.abort_handle.clone()
    }

    pub async fn send_output(&self, output: ChannelOutput) {
        if let Err(error) = self.output_tx.send(output) {
            eprintln!(
                "[{}] failed to send output to ACP plugin bridge: {}",
                self.channel_kind, error
            );
        }
    }

    pub async fn shutdown(&self) {
        self.abort_handle.abort();
    }
}

/// Run the ACP agent-side connection on a dedicated thread.
/// Plugin is ACP Client, we are ACP Agent.
async fn run_acp_plugin_bridge(
    channel_kind: String,
    config: serde_json::Value,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    mut output_rx: mpsc::UnboundedReceiver<ChannelOutput>,
) {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let channel_kind_clone = channel_kind.clone();
            let config_clone = config.clone();
            let input_tx_clone = input_tx.clone();

            // Create ACP AgentSideConnection
            let (conn, handle_io) = acp::AgentSideConnection::new(
                PluginAgentHandler {
                    channel_kind: channel_kind_clone,
                    config: config_clone,
                    input_tx: input_tx_clone,
                },
                stdin.compat_write(),
                stdout.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );

            // Spawn IO handler
            let io_channel = channel_kind.clone();
            tokio::task::spawn_local(async move {
                if let Err(error) = handle_io.await {
                    eprintln!("[{}] ACP plugin IO terminated: {}", io_channel, error);
                }
            });

            // Forward ChannelOutput → ACP Client methods
            let fwd_channel = channel_kind.clone();
            tokio::task::spawn_local(async move {
                while let Some(output) = output_rx.recv().await {
                    forward_output_to_plugin(&conn, &fwd_channel, output).await;
                }
            });

            // Keep alive until connection closes
            std::future::pending::<()>().await;
        })
        .await;
}

/// Forward a ChannelOutput to the plugin via ACP protocol.
async fn forward_output_to_plugin(
    conn: &acp::AgentSideConnection,
    channel_kind: &str,
    output: ChannelOutput,
) {
    match output {
        ChannelOutput::RawAcp { route, payload } => {
            // RawAcp is a serialized SessionNotification from the real agent.
            // Replace the real agent's sessionId with the chatId the plugin expects.
            match serde_json::from_value::<acp::SessionNotification>(payload) {
                Ok(mut notification) => {
                    // Translate: real session ID → chat ID (what plugin sent as sessionId)
                    notification.session_id = route.chat_id.clone().into();
                    if let Err(error) = acp::Client::session_notification(&*conn, notification).await {
                        eprintln!(
                            "[{}] failed to send session_notification: {}",
                            channel_kind, error
                        );
                    }
                }
                Err(error) => {
                    eprintln!(
                        "[{}] failed to parse RawAcp as SessionNotification: {}",
                        channel_kind, error
                    );
                }
            }
        }
        ChannelOutput::SystemText { text, .. } => {
            send_ext_notification(conn, channel_kind, "channel/system_text", &serde_json::json!({ "text": text })).await;
        }
        ChannelOutput::AgentReady {
            agent, version, ..
        } => {
            send_ext_notification(conn, channel_kind, "channel/agent_ready", &serde_json::json!({
                "agent": agent,
                "version": version,
            })).await;
        }
        ChannelOutput::SessionReady { session_id, .. } => {
            send_ext_notification(conn, channel_kind, "channel/session_ready", &serde_json::json!({
                "sessionId": session_id,
            })).await;
        }
    }
}

async fn send_ext_notification(
    conn: &acp::AgentSideConnection,
    channel_kind: &str,
    method: &str,
    params: &serde_json::Value,
) {
    let raw_params: Arc<RawValue> = match RawValue::from_string(serde_json::to_string(params).unwrap_or_default()) {
        Ok(raw) => Arc::from(raw),
        Err(error) => {
            eprintln!("[{}] failed to serialize ext params: {}", channel_kind, error);
            return;
        }
    };
    let notification = acp::ExtNotification::new(method, raw_params);
    if let Err(error) = acp::Client::ext_notification(&*conn, notification).await {
        eprintln!("[{}] failed to send ext_notification {}: {}", channel_kind, method, error);
    }
}

/// ACP Agent handler for a channel plugin.
/// Plugin calls prompt() → we convert to ChannelInput and route to ACPHub.
struct PluginAgentHandler {
    channel_kind: String,
    config: serde_json::Value,
    input_tx: mpsc::UnboundedSender<ChannelInput>,
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for PluginAgentHandler {
    async fn initialize(&self, _args: acp::InitializeRequest) -> acp::Result<acp::InitializeResponse> {
        eprintln!("[{}] ACP initialize from plugin", self.channel_kind);

        let mut meta = serde_json::Map::new();
        meta.insert("channelKind".into(), self.channel_kind.clone().into());
        meta.insert("config".into(), self.config.clone());
        meta.insert("hostVersion".into(), env!("CARGO_PKG_VERSION").into());

        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_info(
                acp::Implementation::new("vibearound-host", env!("CARGO_PKG_VERSION"))
                    .title("VibeAround"),
            )
            .meta(meta))
    }

    async fn authenticate(&self, _args: acp::AuthenticateRequest) -> acp::Result<acp::AuthenticateResponse> {
        Err(acp::Error::method_not_found())
    }

    async fn new_session(&self, _args: acp::NewSessionRequest) -> acp::Result<acp::NewSessionResponse> {
        // Plugins don't manage sessions — we handle them internally.
        Err(acp::Error::method_not_found())
    }

    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        // Convention: plugin uses chatId as ACP sessionId (see module doc)
        let chat_id = args.session_id.to_string();
        let route = RouteKey::new(&self.channel_kind, &chat_id);

        // Extract text from prompt content blocks
        let text: String = args
            .prompt
            .iter()
            .filter_map(|block| match block {
                acp::ContentBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        if text.is_empty() {
            return Err(acp::Error::invalid_params());
        }

        eprintln!(
            "[{}] ACP prompt chat_id={} text_len={}",
            self.channel_kind, chat_id, text.len()
        );

        // Convert to ChannelInput and send for processing
        let input = ChannelInput::Message {
            envelope: ChannelEnvelope {
                route,
                message_id: uuid::Uuid::new_v4().to_string(),
                turn_id: None,
                text,
                sender_id: format!("{}-user", self.channel_kind),
                attachments: vec![],
                parent_id: None,
                cli_kind: None,
            },
        };

        if let Err(error) = self.input_tx.send(input) {
            eprintln!("[{}] failed to route prompt: {}", self.channel_kind, error);
            return Err(acp::Error::internal_error());
        }

        // Note: prompt response comes asynchronously through ChannelOutput.
        // The streaming events flow back via session_notification on the connection.
        // TODO: wire up proper prompt completion tracking so we return after turn ends
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn cancel(&self, args: acp::CancelNotification) -> acp::Result<()> {
        let chat_id = args.session_id.to_string();
        let route = RouteKey::new(&self.channel_kind, &chat_id);

        eprintln!("[{}] ACP cancel chat_id={}", self.channel_kind, chat_id);

        let _ = self.input_tx.send(ChannelInput::Stop { route });
        Ok(())
    }

    async fn ext_notification(&self, args: acp::ExtNotification) -> acp::Result<()> {
        let method = args.method.to_string();
        let params: serde_json::Value = serde_json::from_str(args.params.get())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        let params_obj = params.as_object().cloned().unwrap_or_default();

        match method.as_str() {
            "channel/callback" => {
                let channel_id = params_obj
                    .get("channelId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let chat_id = channel_id
                    .strip_prefix(&format!("{}:", self.channel_kind))
                    .unwrap_or(channel_id);
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
            "channel/close" => {
                let chat_id = params_obj
                    .get("chatId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let route = RouteKey::new(&self.channel_kind, chat_id);
                let reason = params_obj
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let _ = self.input_tx.send(ChannelInput::Close { route, reason });
            }
            other => {
                eprintln!(
                    "[{}] unhandled ext_notification: {}",
                    self.channel_kind, other
                );
            }
        }
        Ok(())
    }
}
