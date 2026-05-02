//! ConversationManager: the routing layer.
//!
//! Holds one [`Conversation`] per [`RouteKey`] and dispatches system
//! commands (prompt / cancel / close / switch_agent / ...) to it. Emits
//! two broadcast streams:
//!
//! - `event_tx` — typed [`SystemEvent`] for lifecycle milestones consumed
//!   by `channel_manager`.
//! - `change_tx` — untyped `()` ping for dashboard-style consumers that
//!   re-poll `list()` on each signal. Exposed via [`StateSource`].
//!
//! The real per-conversation state lives on [`Conversation`]; this type
//! is just the table + dispatcher.
//!
//! [`StateSource`]: crate::state::StateSource

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::broadcast;

use crate::agent::AgentClientHandler;
use crate::routing::RouteKey;

use agent_client_protocol as acp;

use self::conversation::Conversation;

pub mod conversation;
pub mod event;
pub mod handover;
mod session_log;

pub use conversation::ConversationState;
pub use event::SystemEvent;

pub struct ConversationManager {
    conversations: DashMap<RouteKey, Arc<Conversation>>,
    event_tx: broadcast::Sender<SystemEvent>,
    change_tx: broadcast::Sender<()>,
}

impl ConversationManager {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let (change_tx, _) = broadcast::channel(256);
        Self {
            conversations: DashMap::new(),
            event_tx,
            change_tx,
        }
    }

    /// Subscribe to typed lifecycle events.
    pub fn subscribe(&self) -> broadcast::Receiver<SystemEvent> {
        self.event_tx.subscribe()
    }

    /// List every currently-held conversation. Consumers read each
    /// conversation's live state via `conv.state().await`; immutable
    /// getters (`route` field, `started_at()`, `bot_identity()`) don't
    /// need the state snapshot.
    pub fn list(&self) -> Vec<Arc<Conversation>> {
        self.conversations.iter().map(|e| Arc::clone(e.value())).collect()
    }

    /// Look up a conversation by route. Returns `None` if none has been
    /// created for that route yet.
    pub fn conversation(&self, route: &RouteKey) -> Option<Arc<Conversation>> {
        self.conversations.get(route).map(|e| Arc::clone(&e))
    }

    // -----------------------------------------------------------------------
    // Command dispatch — direct methods, no command enums
    // -----------------------------------------------------------------------

    /// Send a prompt on a route. Handles agent spawn and session creation
    /// transparently on first call.
    pub async fn prompt(
        &self,
        route: RouteKey,
        cli_kind: Option<String>,
        content_blocks: Vec<acp::ContentBlock>,
        handler: Arc<dyn AgentClientHandler>,
    ) -> acp::Result<acp::PromptResponse> {
        let conv = self.get_or_create(route);
        conv.prompt(cli_kind, content_blocks, handler).await
    }

    /// Cancel the active turn on a route.
    pub async fn cancel(&self, route: &RouteKey) -> acp::Result<()> {
        let Some(conv) = self.get(route) else {
            return Err(acp::Error::method_not_found());
        };
        conv.cancel().await
    }

    /// Close a route — kill agent, remove conversation.
    pub async fn close(&self, route: &RouteKey, reason: Option<String>) {
        if let Some((_, conv)) = self.conversations.remove(route) {
            conv.close(reason).await;
            let _ = self.change_tx.send(());
        }
    }

    /// Switch agent kind on a route (creates conversation if needed).
    pub async fn switch_agent(
        &self,
        route: &RouteKey,
        agent_kind: String,
    ) -> anyhow::Result<String> {
        let agent_kind =
            crate::resources::resolve_agent_id(&agent_kind).map_err(anyhow::Error::msg)?;
        let conv = self.get_or_create(route.clone());
        conv.switch_agent(agent_kind).await
    }

    /// Switch profile on a route (creates conversation if needed).
    pub async fn switch_profile(&self, route: &RouteKey, profile: String) {
        let conv = self.get_or_create(route.clone());
        conv.switch_profile(profile).await;
    }

    /// Reset session on a route (new conversation thread, same agent).
    pub async fn reset_session(&self, route: &RouteKey) {
        if let Some(conv) = self.get(route) {
            conv.reset_session().await;
        }
    }

    /// Set the permission mode of the current session on a route.
    /// Returns an error if there is no active session yet.
    pub async fn set_session_mode(
        &self,
        route: &RouteKey,
        mode_id: String,
    ) -> acp::Result<()> {
        let conv = self
            .get(route)
            .ok_or_else(acp::Error::method_not_found)?;
        conv.set_session_mode(mode_id).await
    }

    /// Get cached available agent commands for a route.
    pub async fn list_agent_commands(&self, route: &RouteKey) -> serde_json::Value {
        match self.get(route) {
            Some(conv) => conv.list_agent_commands().await,
            None => serde_json::Value::Array(vec![]),
        }
    }

    /// Update cached agent commands for a route (called on available_commands_update).
    pub async fn list_agent_commands_update(&self, route: &RouteKey, commands: serde_json::Value) {
        if let Some(conv) = self.get(route) {
            conv.update_agent_commands(commands).await;
        }
    }

    /// Shutdown all routes.
    pub async fn shutdown_all(&self) {
        let routes: Vec<RouteKey> = self.conversations.iter().map(|e| e.key().clone()).collect();
        for route in routes {
            self.close(&route, Some("shutdown".to_string())).await;
        }
    }

    // -----------------------------------------------------------------------
    // Session handover — conversation lifecycle only
    // -----------------------------------------------------------------------

    /// Prepare a conversation for session pickup — set the
    /// resume_session_id and cwd so the next prompt spawns an agent that
    /// loads the given session.
    pub async fn prepare_pickup(
        &self,
        route: RouteKey,
        cli_kind: String,
        resume_session_id: String,
        cwd: Option<String>,
    ) -> anyhow::Result<()> {
        let conv = self.get_or_create(route);
        conv.set_handover(cli_kind, resume_session_id, cwd).await
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    fn get(&self, route: &RouteKey) -> Option<Arc<Conversation>> {
        self.conversations.get(route).map(|e| Arc::clone(&e))
    }

    fn get_or_create(&self, route: RouteKey) -> Arc<Conversation> {
        if let Some(existing) = self.get(&route) {
            return existing;
        }

        let conv = Arc::new(Conversation::new(
            route.clone(),
            self.event_tx.clone(),
            self.change_tx.clone(),
        ));
        self.conversations.insert(route.clone(), Arc::clone(&conv));
        let _ = self.event_tx.send(SystemEvent::RouteCreated { route });
        let _ = self.change_tx.send(());
        conv
    }
}

impl Default for ConversationManager {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::state::StateSource for ConversationManager {
    type Entry = Arc<Conversation>;

    async fn list(&self) -> Vec<Self::Entry> {
        self.list()
    }

    fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }
}
