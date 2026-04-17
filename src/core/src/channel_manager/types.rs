//! Wire types for the legacy stdio plugin transport.
//!
//! These structs flow between the host and stdio plugins via JSON. They
//! pre-date the ACP-native path that `ws_chat` uses today, but they are still
//! the common currency for every plugin subprocess.

use serde::{Deserialize, Serialize};

use crate::acp::routing::{Attachment, MessageId, RouteEnvelope, RouteKey, TurnId};

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
    CommandMenu {
        route: RouteKey,
        system_commands: serde_json::Value,
        agent_commands: serde_json::Value,
    },
    /// Forward a `requestPermission` ACP call from the upstream agent down to
    /// the plugin. The plugin answers via its `client.requestPermission`
    /// handler (standard ACP), and the forwarder task sends the response back
    /// via the oneshot registered in `PluginHost::pending_permissions`.
    ///
    /// `request_id` matches the entry in `pending_permissions`.
    /// `payload` is a JSON-serialized `acp::RequestPermissionRequest`.
    PermissionRequest {
        route: RouteKey,
        request_id: String,
        payload: serde_json::Value,
    },
}

impl ChannelOutput {
    pub fn route_key(&self) -> &RouteKey {
        match self {
            Self::RawAcp { route, .. }
            | Self::SystemText { route, .. }
            | Self::AgentReady { route, .. }
            | Self::SessionReady { route, .. }
            | Self::CommandMenu { route, .. }
            | Self::PermissionRequest { route, .. } => route,
        }
    }
}
