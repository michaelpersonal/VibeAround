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
//!
//! ## Prompt lifecycle
//!
//! Plugin calls `prompt()` → host calls `acp_hub.prompt()` directly →
//! session notifications stream to plugin during processing →
//! `prompt()` returns the real `PromptResponse` with actual `StopReason`.

use std::sync::Arc;

use serde_json::value::RawValue;
use tokio::io::AsyncBufReadExt;

use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use agent_client_protocol as acp;

use super::manifest::ChannelPluginManifest;
use super::plugin_host::PluginHost;
use super::{handle_prompt, ChannelEnvelope, ChannelInput, ChannelOutput};
use crate::acp::routing::RouteKey;
use crate::acp_hub::ACPHub;
use crate::child_registry::{ChildKind, ChildRegistry};

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
        acp_hub: Arc<ACPHub>,
        plugin_host: Arc<PluginHost>,
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

        let mut child = crate::env::command("node")
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

        // Register in the global ChildRegistry. This is the canonical owner
        // of the `Child` handle now — the registry guarantees the process is
        // SIGKILLed on daemon stop + Tauri RunEvent::Exit even if the tokio
        // runtime tears down without polling this task's destructor.
        let registry_id = ChildRegistry::global().register(
            ChildKind::Plugin,
            channel_kind.clone(),
            child,
        );

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

        // Guardian task — on abort (via `shutdown()` → `abort_handle.abort()`)
        // it removes the child from the registry and drops it, triggering
        // `kill_on_drop`. This is the happy-path shutdown.
        //
        // If the runtime tears down abruptly instead, the ChildRegistry
        // kill_all() path (fired from `RunningDaemon::stop` and Tauri Exit)
        // still kills the process synchronously. Either way, no orphans.
        let guardian_channel = channel_kind.clone();
        let guardian_task = tokio::spawn(async move {
            // Park forever; abort triggers the drop handler below.
            struct Guard(u64, String);
            impl Drop for Guard {
                fn drop(&mut self) {
                    if let Some(_child) = ChildRegistry::global().remove(self.0) {
                        // `_child` dropped here → kill_on_drop fires.
                        eprintln!(
                            "[{}] guardian drop: plugin child killed via registry",
                            self.1
                        );
                    }
                }
            }
            let _guard = Guard(registry_id, guardian_channel);
            std::future::pending::<()>().await;
        });
        let abort_handle = guardian_task.abort_handle();

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
                    run_acp_plugin_bridge(
                        acp_channel,
                        raw_config,
                        stdin,
                        stdout,
                        input_tx,
                        output_rx,
                        acp_hub,
                        plugin_host,
                    )
                    .await;
                });
            })
            .map_err(|e| format!("Failed to spawn plugin thread: {}", e))?;

        Ok(Self {
            channel_kind,
            output_tx,
            abort_handle,
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
    acp_hub: Arc<ACPHub>,
    plugin_host: Arc<PluginHost>,
) {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // Create ACP AgentSideConnection
            let (conn, handle_io) = acp::AgentSideConnection::new(
                PluginAgentHandler {
                    channel_kind: channel_kind.clone(),
                    config: config.clone(),
                    input_tx: input_tx.clone(),
                    acp_hub,
                    plugin_host,
                },
                stdin.compat_write(),
                stdout.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );

            // Forward ChannelOutput → ACP Client methods.
            // Spawn so it runs concurrently with handle_io below.
            let fwd_channel = channel_kind.clone();
            let forwarder = tokio::task::spawn_local(async move {
                eprintln!("[{}] output forwarder started", fwd_channel);
                while let Some(output) = output_rx.recv().await {
                    forward_output_to_plugin(&conn, &fwd_channel, output).await;
                }
                eprintln!("[{}] output forwarder ended", fwd_channel);
            });

            // Drive ACP IO on this task. When the child plugin process dies
            // its stdin/stdout close, `handle_io` returns, and the LocalSet
            // exits — letting the block_on runtime drop, the std::thread
            // exit, and the bridge thread's resources release cleanly.
            if let Err(error) = handle_io.await {
                eprintln!("[{}] ACP plugin IO terminated: {}", channel_kind, error);
            }
            forwarder.abort();
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
            match serde_json::from_value::<acp::SessionNotification>(payload.clone()) {
                Ok(mut notification) => {
                    notification.session_id = route.chat_id.clone().into();
                    if let Err(error) =
                        acp::Client::session_notification(&*conn, notification).await
                    {
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
        ChannelOutput::SystemText { route, text, .. } => {
            send_ext_notification(
                conn,
                channel_kind,
                "va/system_text",
                &serde_json::json!({
                    "chatId": route.chat_id,
                    "text": text,
                }),
            )
            .await;
        }
        ChannelOutput::AgentReady {
            route, agent, version, ..
        } => {
            send_ext_notification(
                conn,
                channel_kind,
                "va/agent_ready",
                &serde_json::json!({
                    "chatId": route.chat_id,
                    "agent": agent,
                    "version": version,
                }),
            )
            .await;
        }
        ChannelOutput::SessionReady { route, session_id, .. } => {
            send_ext_notification(
                conn,
                channel_kind,
                "va/session_ready",
                &serde_json::json!({
                    "chatId": route.chat_id,
                    "sessionId": session_id,
                }),
            )
            .await;
        }
        ChannelOutput::CommandMenu { route, system_commands, agent_commands } => {
            send_ext_notification(
                conn,
                channel_kind,
                "va/command_menu",
                &serde_json::json!({
                    "chatId": route.chat_id,
                    "systemCommands": system_commands,
                    "agentCommands": agent_commands,
                }),
            )
            .await;
        }
    }
}

async fn send_ext_notification(
    conn: &acp::AgentSideConnection,
    channel_kind: &str,
    method: &str,
    params: &serde_json::Value,
) {
    let raw_params: Arc<RawValue> =
        match RawValue::from_string(serde_json::to_string(params).unwrap_or_default()) {
            Ok(raw) => Arc::from(raw),
            Err(error) => {
                eprintln!(
                    "[{}] failed to serialize ext params: {}",
                    channel_kind, error
                );
                return;
            }
        };
    let notification = acp::ExtNotification::new(method, raw_params);
    if let Err(error) = acp::Client::ext_notification(&*conn, notification).await {
        eprintln!(
            "[{}] failed to send ext_notification {}: {}",
            channel_kind, method, error
        );
    }
}

/// ACP Agent handler for a channel plugin.
/// `prompt()` calls through to `handle_prompt()` directly — blocks until
/// the turn completes and returns the real `PromptResponse` with `StopReason`.
struct PluginAgentHandler {
    channel_kind: String,
    config: serde_json::Value,
    /// Still used for fire-and-forget operations: cancel, callback.
    input_tx: mpsc::UnboundedSender<ChannelInput>,
    acp_hub: Arc<ACPHub>,
    plugin_host: Arc<PluginHost>,
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for PluginAgentHandler {
    async fn initialize(
        &self,
        _args: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        eprintln!("[{}] ACP initialize from plugin", self.channel_kind);

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

        Ok(
            acp::InitializeResponse::new(acp::ProtocolVersion::V1)
                .agent_info(
                    acp::Implementation::new("vibearound-host", env!("CARGO_PKG_VERSION"))
                        .title("VibeAround"),
                )
                .meta(meta),
        )
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

        eprintln!(
            "[{}] ACP prompt chat_id={} blocks={} text_preview={}",
            self.channel_kind,
            chat_id,
            content_blocks.len(),
            &text_preview[..text_preview.len().min(80)]
        );

        // Call through to handle_prompt — blocks until the turn completes.
        // Session notifications stream to the plugin via ChannelBridgeHandler
        // → PluginHost → output_tx → output forwarder → conn.session_notification().
        let result = handle_prompt(
            &self.acp_hub,
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
                .send_output(crate::channel_manager::ChannelOutput::SystemText {
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

        eprintln!("[{}] ACP cancel chat_id={}", self.channel_kind, chat_id);

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
            "va/callback" => {
                // Accept both chatId (new) and channelId (legacy, "kind:chatId") for compat.
                let chat_id = params_obj
                    .get("chatId")
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        params_obj.get("channelId").and_then(|v| v.as_str()).map(|cid| {
                            cid.strip_prefix(&format!("{}:", self.channel_kind)).unwrap_or(cid)
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
                eprintln!(
                    "[{}] unhandled ext_notification: {}",
                    self.channel_kind, other
                );
            }
        }
        Ok(())
    }

    async fn ext_method(&self, args: acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let method = args.method.to_string();
        eprintln!("[{}] unhandled ext_method: {}", self.channel_kind, method);
        Err(acp::Error::method_not_found())
    }
}
