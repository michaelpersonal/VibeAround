use std::fmt;

use serde::{Deserialize, Serialize};

/// Channel kind identifier (e.g. "web", "telegram").
pub type ChannelKind = String;

/// Bot identity on the IM platform (e.g. Feishu botOpenId, Telegram bot username).
pub type BotId = String;

/// Chat identifier within a channel.
pub type ChatId = String;

/// Platform envelope identifier.
pub type MessageId = String;

/// ACP/provider session identifier.
pub type SessionId = String;

/// External CLI session identifier.
pub type CliSessionId = String;

/// Runtime instance identifier for a route.
pub type RuntimeId = String;

/// Logical turn identifier on a route.
pub type TurnId = String;

/// Stable route key for a conversation path through a channel.
///
/// The triple `(channel_kind, bot_id, chat_id)` uniquely identifies a bot
/// instance in a chat. This supports group chats with multiple bots — each
/// bot has its own route.
///
/// `bot_id` defaults to `channel_kind` for backward compat with plugins
/// that haven't reported their IM identity yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteKey {
    pub channel_kind: ChannelKind,
    /// Bot identity on the IM platform. Defaults to `channel_kind`.
    /// Each plugin process represents one bot; future multi-bot support
    /// would use separate plugin processes with distinct bot_id values.
    #[serde(default)]
    pub bot_id: BotId,
    pub chat_id: ChatId,
}

impl RouteKey {
    pub fn new(channel_kind: impl Into<ChannelKind>, chat_id: impl Into<ChatId>) -> Self {
        let ck: ChannelKind = channel_kind.into();
        Self {
            bot_id: ck.clone(),
            channel_kind: ck,
            chat_id: chat_id.into(),
        }
    }

    pub fn with_bot_id(
        channel_kind: impl Into<ChannelKind>,
        bot_id: impl Into<BotId>,
        chat_id: impl Into<ChatId>,
    ) -> Self {
        Self {
            channel_kind: channel_kind.into(),
            bot_id: bot_id.into(),
            chat_id: chat_id.into(),
        }
    }

    /// Serialized form: `channel_kind:chat_id` (backward compat).
    /// Does NOT include bot_id — the key format is used for display and
    /// dashboard routing where bot_id isn't needed yet.
    pub fn as_key(&self) -> String {
        format!("{}:{}", self.channel_kind, self.chat_id)
    }

    pub fn from_key(key: &str) -> Option<Self> {
        let (channel_kind, chat_id) = key.split_once(':')?;
        Some(Self::new(channel_kind, chat_id))
    }
}

impl fmt::Display for RouteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.channel_kind, self.chat_id)
    }
}

/// Attachment metadata carried with a routed envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub message_id: String,
    pub file_key: String,
    pub file_name: String,
    pub resource_type: String,
}

/// Shared routed envelope exchanged between channel/session/agent layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEnvelope {
    pub channel_kind: ChannelKind,
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub turn_id: Option<TurnId>,
    pub text: String,
    pub sender_id: String,
    pub attachments: Vec<Attachment>,
    pub parent_id: Option<String>,
    pub cli_kind: Option<String>,
}

impl RouteEnvelope {
    pub fn route_key(&self) -> RouteKey {
        RouteKey::new(self.channel_kind.clone(), self.chat_id.clone())
    }
}

/// Runtime/session identity for an agent attached to a route.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentSessionRef {
    pub route: RouteKey,
    pub runtime_id: RuntimeId,
    pub session_id: Option<SessionId>,
    pub cli_kind: Option<String>,
    pub cli_session_id: Option<CliSessionId>,
    pub profile: Option<String>,
}

impl AgentSessionRef {
    pub fn new(route: RouteKey, runtime_id: impl Into<RuntimeId>) -> Self {
        Self {
            route,
            runtime_id: runtime_id.into(),
            session_id: None,
            cli_kind: None,
            cli_session_id: None,
            profile: None,
        }
    }
}
