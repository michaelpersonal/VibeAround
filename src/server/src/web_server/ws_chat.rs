//! WebSocket handler for web chat channel.
//!
//! - GET /ws/chat — ACP-native websocket adapter
//!
//! Inbound user messages are dispatched to ACPHub via the channel-input
//! thread (fire-and-forget through ChannelManager).  ACP events flow back
//! through the WebChannelManager outbound channel to the websocket.

use axum::extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use uuid::Uuid;

use common::acp::routing::RouteKey;
use common::channel_manager::{ChannelEnvelope, ChannelInput, ChannelOutput};
use common::config;

use super::AppState;

/// WebSocket upgrade handler for web chat.
pub async fn ws_chat_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_chat_socket(socket, state))
}

async fn handle_chat_socket(socket: WebSocket, state: AppState) {
    let chat_id = Uuid::new_v4().to_string();
    let channel_id = format!("web:{}", chat_id);
    let route = RouteKey::new("web", &chat_id);

    // Register this connection for outbound ACP events
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ChannelOutput>();
    state.web_channel.register_connection(chat_id.clone(), tx);

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Load config
    let cfg = config::ensure_loaded();
    // Try "ws" first (settings.json key), then "web" (internal channel kind)
    let verbose = {
        let v = cfg.channel_verbose("ws");
        if !v.show_thinking && !v.show_tool_use {
            cfg.channel_verbose("web")
        } else {
            v
        }
    };
    let agents: Vec<serde_json::Value> = cfg
        .enabled_agents
        .iter()
        .map(|kind| {
            serde_json::json!({
                "id": kind.to_string(),
                "name": kind.display_name(),
                "description": kind.description(),
            })
        })
        .collect();
    let config_msg = serde_json::json!({
        "type": "config",
        "channelId": channel_id,
        "agents": agents,
        "default_agent": cfg.default_agent,
    });
    let _ = ws_tx.send(Message::Text(config_msg.to_string().into())).await;

    // Outbound: drain ACP events from WebChannelManager → websocket
    let outbound_task = tokio::spawn(async move {
        while let Some(output) = rx.recv().await {
            let msg = output_to_client_json(output, &verbose);
            if msg.is_null() {
                continue; // filtered by verbose config
            }
            eprintln!("[ws_chat] outbound → ws: {}", msg);
            if ws_tx.send(Message::Text(msg.to_string().into())).await.is_err() {
                break;
            }
        }
    });

    // Inbound: ws messages → ChannelInput → channel-input thread → ACPHub
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                if let Some(input) = parse_channel_input(&chat_id, &text) {
                    state.channel_hub.handle_input(input);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup
    outbound_task.abort();
    state.web_channel.unregister_connection(&chat_id);
    state.channel_hub.acp_hub().close(&route, None).await;
}

// --- PLACEHOLDER_REST ---

fn parse_channel_input(chat_id: &str, text: &str) -> Option<ChannelInput> {
    let parsed = serde_json::from_str::<serde_json::Value>(text);

    match parsed {
        Ok(v) => {
            let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
            match ty {
                "message" => {
                    let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
                    if text.is_empty() {
                        return None;
                    }
                    let message_id = v
                        .get("messageId")
                        .and_then(|x| x.as_str())
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| Uuid::new_v4().to_string());
                    let agent = v
                        .get("agent")
                        .and_then(|x| x.as_str())
                        .map(str::trim)
                        .filter(|x| !x.is_empty());
                    Some(ChannelInput::Message {
                        envelope: ChannelEnvelope {
                            route: RouteKey::new("web", chat_id),
                            message_id,
                            turn_id: None,
                            text: text.to_string(),
                            sender_id: "web-user".to_string(),
                            attachments: vec![],
                            parent_id: None,
                            cli_kind: agent.map(ToOwned::to_owned),
                        },
                    })
                }
                _ => None,
            }
        }
        Err(_) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(ChannelInput::Message {
                    envelope: ChannelEnvelope {
                        route: RouteKey::new("web", chat_id),
                        message_id: Uuid::new_v4().to_string(),
                        turn_id: None,
                        text: trimmed.to_string(),
                        sender_id: "web-user".to_string(),
                        attachments: vec![],
                        parent_id: None,
                        cli_kind: None,
                    },
                })
            }
        }
    }
}

fn output_to_client_json(output: ChannelOutput, verbose: &common::config::ImVerboseConfig) -> serde_json::Value {
    match output {
        ChannelOutput::RawAcp { payload, .. } => acp_to_frontend(payload, verbose),
        ChannelOutput::SystemText { text, .. } => {
            serde_json::json!({ "kind": "text", "text": text })
        }
        ChannelOutput::AgentReady {
            agent, version, ..
        } => {
            serde_json::json!({
                "kind": "agent_ready",
                "agent": agent,
                "version": version,
            })
        }
        ChannelOutput::SessionReady {
            session_id, ..
        } => {
            serde_json::json!({
                "kind": "session_ready",
                "sessionId": session_id,
            })
        }
        ChannelOutput::CommandMenu {
            system_commands, agent_commands, ..
        } => {
            serde_json::json!({
                "kind": "command_menu",
                "systemCommands": system_commands,
                "agentCommands": agent_commands,
            })
        }
        ChannelOutput::PermissionRequest {
            request_id, payload, ..
        } => {
            // Forward to web client so it can render an approval UI. Client is
            // expected to post `request_id` + selected `optionId` back via the
            // websocket. If the client doesn't implement it yet, the
            // corresponding pending oneshot in PluginHost will wait forever —
            // which matches the "no timeout" UX for IM channels.
            serde_json::json!({
                "kind": "permission_request",
                "requestId": request_id,
                "request": payload,
            })
        }
    }
}

/// Translate ACP session_notification payload into the frontend's expected format.
///
/// ACP shape:  { "sessionId": "...", "update": { "sessionUpdate": "<variant>", ... } }
/// Frontend expects:  { "kind": "token"|"thinking"|"tool_use"|"tool_result"|"turn_complete"|"error", ... }
fn acp_to_frontend(payload: serde_json::Value, verbose: &common::config::ImVerboseConfig) -> serde_json::Value {
    let update = match payload.get("update") {
        Some(u) => u,
        None => return payload, // not a session_notification, pass through
    };

    let variant = update
        .get("sessionUpdate")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match variant {
        "agent_message_chunk" => {
            let text = update
                .pointer("/content/text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            serde_json::json!({ "kind": "token", "delta": text })
        }
        "agent_thought_chunk" => {
            if !verbose.show_thinking {
                return serde_json::Value::Null;
            }
            let text = update
                .pointer("/content/text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            serde_json::json!({ "kind": "thinking", "text": text })
        }
        "tool_call_update" => {
            if !verbose.show_tool_use {
                return serde_json::Value::Null;
            }
            let title = update
                .pointer("/fields/title")
                .and_then(|v| v.as_str())
                .unwrap_or("tool");
            let status = update
                .pointer("/fields/status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match status {
                "completed" | "error" => serde_json::json!({ "kind": "tool_result" }),
                _ => serde_json::json!({ "kind": "tool_use", "tool": title }),
            }
        }
        "turn_complete" => {
            serde_json::json!({ "kind": "turn_complete" })
        }
        "error" => {
            let text = update
                .pointer("/content/text")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            serde_json::json!({ "kind": "error", "error": text })
        }
        other => {
            eprintln!("[ws_chat] unhandled ACP variant: {:?}", other);
            return serde_json::json!({ "kind": "debug", "variant": other, "raw": payload });
        }
    }
}

