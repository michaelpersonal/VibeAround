//! `ChannelOutput` → plugin dispatch.
//!
//! Each variant of `ChannelOutput` maps to a different ACP Client call:
//!
//! - `RawAcp`             → `session_notification` (after rewriting session_id)
//! - `SystemText`         → `ext_notification("va/system_text", ...)`
//! - `AgentReady`         → `ext_notification("va/agent_ready", ...)`
//! - `SessionReady`       → `ext_notification("va/session_ready", ...)`
//! - `CommandMenu`        → `ext_notification("va/command_menu", ...)`
//! - `PermissionRequest`  → real `request_permission` call; response is
//!   routed back through `PluginHost::pending_permissions`.

use std::sync::Arc;

use serde_json::value::RawValue;

use agent_client_protocol as acp;

use super::super::plugin_host::PluginHost;
use super::super::ChannelOutput;

/// Forward a `ChannelOutput` to the plugin via the ACP Client API.
pub(super) async fn forward_output_to_plugin(
    conn: &acp::AgentSideConnection,
    channel_kind: &str,
    plugin_host: &Arc<PluginHost>,
    output: ChannelOutput,
) {
    match output {
        ChannelOutput::RawAcp { route, payload } => {
            match serde_json::from_value::<acp::SessionNotification>(payload.clone()) {
                Ok(mut notification) => {
                    notification.session_id = route.chat_id.clone().into();
                    if let Err(error) =
                        acp::Client::session_notification(conn, notification).await
                    {
                        tracing::info!(
                            "[{}] failed to send session_notification: {}",
                            channel_kind, error
                        );
                    }
                }
                Err(error) => {
                    tracing::info!(
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
            route,
            agent,
            version,
            ..
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
        ChannelOutput::CommandMenu {
            route,
            system_commands,
            agent_commands,
        } => {
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
        ChannelOutput::PermissionRequest {
            route,
            request_id,
            payload,
        } => {
            // Deserialize back to a typed ACP request, then forward as a real
            // ACP `requestPermission` call. The plugin's client-side handler
            // (channel-sdk/plugin.ts → renderer.requestPermission) replies,
            // and we push the response onto the waiting oneshot.
            let request: acp::RequestPermissionRequest = match serde_json::from_value(payload) {
                Ok(r) => r,
                Err(e) => {
                    tracing::info!(
                        "[{}] failed to parse PermissionRequest payload route={} request_id={}: {}",
                        channel_kind, route, request_id, e
                    );
                    if let Some((_, (_, tx))) = plugin_host.pending_permissions.remove(&request_id) {
                        let _ = tx.send(acp::RequestPermissionResponse::new(
                            acp::RequestPermissionOutcome::Cancelled,
                        ));
                    }
                    return;
                }
            };
            let response = acp::Client::request_permission(conn, request).await;
            let Some((_, (_, tx))) = plugin_host.pending_permissions.remove(&request_id) else {
                tracing::info!(
                    "[{}] PermissionRequest response dropped — no pending route={} request_id={}",
                    channel_kind, route, request_id
                );
                return;
            };
            match response {
                Ok(resp) => {
                    let _ = tx.send(resp);
                }
                Err(e) => {
                    tracing::info!(
                        "[{}] plugin requestPermission failed route={} request_id={}: {}",
                        channel_kind, route, request_id, e
                    );
                    let _ = tx.send(acp::RequestPermissionResponse::new(
                        acp::RequestPermissionOutcome::Cancelled,
                    ));
                }
            }
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
                tracing::info!(
                    "[{}] failed to serialize ext params: {}",
                    channel_kind, error
                );
                return;
            }
        };
    let notification = acp::ExtNotification::new(method, raw_params);
    if let Err(error) = acp::Client::ext_notification(conn, notification).await {
        tracing::info!(
            "[{}] failed to send ext_notification {}: {}",
            channel_kind, method, error
        );
    }
}
