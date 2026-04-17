//! `ChannelBridgeHandler` — the downstream handler wired to the upstream
//! `AcpBridge`. Its two jobs:
//!
//! 1. **`session_notification`** — forward every `acp::SessionNotification`
//!    from the agent to the channel plugin as a `ChannelOutput::RawAcp`
//!    envelope. It also peeks at `available_commands_update` events so the
//!    pod can cache the agent's command list for later `/agent` queries.
//! 2. **`request_permission`** — turn an ACP `requestPermission` call from
//!    the upstream agent into a `ChannelOutput::PermissionRequest` to the
//!    plugin, then await the plugin's reply via a per-request oneshot
//!    registered in `PluginHost::pending_permissions`. No timeout — the UX
//!    is "user takes as long as they need".

use std::sync::Arc;

use agent_client_protocol as acp;

use crate::acp::routing::RouteKey;
use crate::acp_hub::ACPHub;
use crate::agent_factory::runtime::BridgeClientHandler;

use super::plugin_host::PluginHost;
use super::types::ChannelOutput;

pub(crate) struct ChannelBridgeHandler {
    plugin_host: Arc<PluginHost>,
    acp_hub: Arc<ACPHub>,
    route: RouteKey,
}

impl ChannelBridgeHandler {
    pub(crate) fn new(
        plugin_host: Arc<PluginHost>,
        acp_hub: Arc<ACPHub>,
        route: RouteKey,
    ) -> Self {
        Self { plugin_host, acp_hub, route }
    }
}

#[async_trait::async_trait(?Send)]
impl BridgeClientHandler for ChannelBridgeHandler {
    async fn session_notification(&self, args: acp::SessionNotification) -> acp::Result<()> {
        // Cache available_commands_update in the pod for later query.
        let payload = serde_json::to_value(&args)
            .map_err(|e| acp::Error::new(-32603, format!("serialize: {}", e)))?;

        // Log the update variant so we can tell whether an agent is emitting
        // real assistant text or only tool/thinking chunks. Claude Agent
        // v0.25.x sometimes end-turns after only tool_call updates and never
        // yields a user-visible message; this log surfaces that case.
        let update_kind = payload
            .get("update")
            .and_then(|u| u.get("sessionUpdate"))
            .and_then(|v| v.as_str())
            .unwrap_or("<none>");
        let preview = payload
            .get("update")
            .and_then(|u| u.get("content"))
            .and_then(|c| c.get("text"))
            .and_then(|v| v.as_str())
            .map(|s| s.chars().take(60).collect::<String>())
            .unwrap_or_default();
        eprintln!(
            "[ChannelBridgeHandler] session_notification route={} session={} kind={} preview={:?}",
            self.route, args.session_id, update_kind, preview
        );

        if let Some(update) = payload.get("update") {
            if update.get("sessionUpdate").and_then(|v| v.as_str())
                == Some("available_commands_update")
            {
                if let Some(commands) = update.get("availableCommands") {
                    self.acp_hub
                        .list_agent_commands_update(&self.route, commands.clone())
                        .await;
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
        if args.options.is_empty() {
            return Err(acp::Error::method_not_found());
        }

        // Rewrite the upstream session_id → plugin-facing chat_id before
        // forwarding. Plugins see chat_id as their ACP session id.
        let mut forwarded = args;
        forwarded.session_id = self.route.chat_id.clone().into();

        // Register a oneshot keyed by a fresh request_id. The plugin-bridge
        // forwarder task consumes it once the plugin's ACP response arrives.
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel::<acp::RequestPermissionResponse>();
        self.plugin_host.pending_permissions.insert(request_id.clone(), tx);

        let payload = match serde_json::to_value(&forwarded) {
            Ok(v) => v,
            Err(e) => {
                self.plugin_host.pending_permissions.remove(&request_id);
                return Err(acp::Error::new(
                    -32603,
                    format!("serialize requestPermission: {}", e),
                ));
            }
        };

        eprintln!(
            "[ChannelBridgeHandler] request_permission forwarding route={} request_id={} options={}",
            self.route, request_id, forwarded.options.len()
        );

        self.plugin_host
            .send_output(ChannelOutput::PermissionRequest {
                route: self.route.clone(),
                request_id: request_id.clone(),
                payload,
            })
            .await;

        // Wait for plugin response — no timeout by design. If the plugin
        // crashes, `tx` is dropped and `rx.await` errors, which we treat as
        // cancelled so the upstream agent turn gracefully ends.
        match rx.await {
            Ok(response) => Ok(response),
            Err(_) => {
                self.plugin_host.pending_permissions.remove(&request_id);
                eprintln!(
                    "[ChannelBridgeHandler] request_permission dropped (plugin gone?) route={} request_id={}",
                    self.route, request_id
                );
                Ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Cancelled,
                ))
            }
        }
    }
}
