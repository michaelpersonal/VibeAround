//! `Conversation` — per-route conversation state + Agent lifecycle coordinator.
//!
//! Owns at most one [`Agent`] at a time. The Agent dies and respawns on
//! `switch_agent` / `full_reset` / crash; this struct holds the state
//! that survives those respawns (cli_kind, profile, session_id, handover,
//! busy/failed, cached commands).
//!
//! ## Module layout
//!
//! - [`state`]             — [`ConversationState`] (mutable runtime fields
//!                            read via `Conversation::state().await`).
//! - [`lifecycle`]         — agent lifecycle (`ensure_agent`,
//!                            `ensure_session`, `full_reset`).
//! - [`media`]             — relocate cached media from staging to the
//!                            session-scoped workspace path before each prompt.
//!
//! Handover plumbing (the `HandoverHandler` replay filter that wraps the
//! downstream handler during `load_session`, and the pickup-code token
//! store) lives next door in [`super::handover`].

mod lifecycle;
mod media;
mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};

use agent_client_protocol as acp;

use crate::agent::{Agent, AgentClientHandler};
use crate::routing::RouteKey;

use super::event::SystemEvent;

use media::relocate_cached_media;

pub use state::ConversationState;

// ---------------------------------------------------------------------------
// Conversation
// ---------------------------------------------------------------------------

pub struct Conversation {
    pub route: RouteKey,
    bot_identity: Option<String>,
    agent: Mutex<Option<Arc<Agent>>>,
    session_id: Mutex<Option<String>>,
    cli_kind: Mutex<Option<String>>,
    profile: Mutex<Option<String>>,
    /// Resolved workspace path, set when agent is spawned.
    workspace: Mutex<Option<String>>,
    initialize: Mutex<Option<acp::InitializeResponse>>,
    busy: Mutex<bool>,
    failed: Mutex<Option<String>>,
    started_at: u64,
    event_tx: broadcast::Sender<SystemEvent>,
    /// Cached available commands from the agent's `available_commands_update` notification.
    agent_commands: Mutex<serde_json::Value>,
    // --- Handover state (consumed once on next prompt) ---
    handover_resume_session_id: Mutex<Option<String>>,
    handover_cwd: Mutex<Option<String>>,
    /// Suppresses session_notification replay during handover load_session.
    /// Released just before the first prompt is sent (not when agent is ready),
    /// because some agents (Gemini) continue replaying after load_session returns.
    suppress_replay: Mutex<Option<Arc<AtomicBool>>>,
    /// Dashboard-facing change ping. Fired on any state mutation that a
    /// consumer of `ConversationManager::subscribe_changes()` would want to
    /// re-poll for. Shared (cloned) from the owning manager's ping channel.
    change_tx: broadcast::Sender<()>,
}

impl Conversation {
    pub fn new(
        route: RouteKey,
        event_tx: broadcast::Sender<SystemEvent>,
        change_tx: broadcast::Sender<()>,
    ) -> Self {
        Self {
            route,
            bot_identity: None,
            agent: Mutex::new(None),
            session_id: Mutex::new(None),
            cli_kind: Mutex::new(None),
            profile: Mutex::new(None),
            workspace: Mutex::new(None),
            initialize: Mutex::new(None),
            busy: Mutex::new(false),
            failed: Mutex::new(None),
            started_at: unix_now_secs(),
            event_tx,
            agent_commands: Mutex::new(serde_json::Value::Array(vec![])),
            handover_resume_session_id: Mutex::new(None),
            handover_cwd: Mutex::new(None),
            suppress_replay: Mutex::new(None),
            change_tx,
        }
    }

    /// Prepare this conversation for a session pickup. Sets cli_kind,
    /// resume_session_id, and optionally cwd so the next prompt spawns an
    /// agent that resumes the given session in the correct workspace.
    pub async fn set_handover(
        &self,
        cli_kind: String,
        resume_session_id: String,
        cwd: Option<String>,
    ) -> anyhow::Result<()> {
        let cli_kind = crate::resources::resolve_agent_id(&cli_kind).map_err(anyhow::Error::msg)?;
        self.full_reset().await;
        *self.cli_kind.lock().await = Some(cli_kind);
        *self.handover_resume_session_id.lock().await = Some(resume_session_id);
        *self.handover_cwd.lock().await = cwd;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Public API — direct methods, no command enums
    // -----------------------------------------------------------------------

    /// Send a prompt to the agent. Handles agent spawn and session creation
    /// transparently on first call.
    pub async fn prompt(
        self: &Arc<Self>,
        cli_kind: Option<String>,
        content_blocks: Vec<acp::ContentBlock>,
        downstream_handler: Arc<dyn AgentClientHandler>,
    ) -> acp::Result<acp::PromptResponse> {
        // No prompt_lock — prompts are forwarded to the agent immediately.
        // CLI agents (Claude Code, Codex, Gemini CLI) accept input at any
        // time and queue/interrupt internally via ACP. Blocking here caused
        // user-visible hangs when a turn didn't end (e.g. background tasks).
        tracing::info!(
            "[Conversation] prompt route={} cli_kind={:?} blocks={}",
            self.route,
            cli_kind,
            content_blocks.len()
        );

        *self.busy.lock().await = true;
        *self.failed.lock().await = None;
        let _ = self.change_tx.send(());

        let result: acp::Result<acp::PromptResponse> = async {
            // Take handover state (consumed once)
            let resume_sid = self.handover_resume_session_id.lock().await.take();
            let resume_cwd = self.handover_cwd.lock().await.take();

            let agent = self
                .ensure_agent(cli_kind, resume_sid, resume_cwd, downstream_handler)
                .await
                .map_err(|error| {
                    tracing::info!(
                        "[Conversation] ensure_agent failed route={}: {:#}",
                        self.route, error
                    );
                    acp::Error::new(-32603, error.to_string())
                })?;

            let session_id = self.ensure_session(&agent).await?;

            // Move cached media files to session-scoped workspace path and update URIs
            let agent_kind = self
                .cli_kind
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| "default".to_string());
            let content_blocks = relocate_cached_media(
                content_blocks,
                &self.route,
                &agent_kind,
                &session_id.to_string(),
            )
            .await;

            // Release suppress_replay now — any lingering history replay from
            // load_session has been swallowed, and the real prompt is about to start.
            if let Some(flag) = self.suppress_replay.lock().await.take() {
                flag.store(false, Ordering::Release);
            }

            tracing::info!(
                "[Conversation] prompt SENDING route={} session={}",
                self.route, session_id
            );
            let request = acp::PromptRequest::new(session_id, content_blocks);
            let response = acp::Agent::prompt(&*agent, request).await;
            tracing::info!(
                "[Conversation] prompt RETURNED route={} ok={}",
                self.route,
                response.is_ok()
            );
            response
        }
        .await;

        *self.busy.lock().await = false;
        if let Err(error) = &result {
            *self.failed.lock().await = Some(error.message.to_string());
        }
        let _ = self.change_tx.send(());

        result
    }

    /// Cancel the active turn.
    pub async fn cancel(&self) -> acp::Result<()> {
        let agent = self
            .agent
            .lock()
            .await
            .clone()
            .ok_or_else(acp::Error::method_not_found)?;
        let session_id = self
            .session_id
            .lock()
            .await
            .clone()
            .ok_or_else(acp::Error::method_not_found)?;
        acp::Agent::cancel(&*agent, acp::CancelNotification::new(session_id)).await
    }

    /// Switch the current session's permission mode. Requires an active
    /// agent + session (caller should ensure this — no auto-spawn because
    /// the mode is a session property that only exists after initialization).
    pub async fn set_session_mode(&self, mode_id: String) -> acp::Result<()> {
        let agent = self
            .agent
            .lock()
            .await
            .clone()
            .ok_or_else(acp::Error::method_not_found)?;
        let session_id = self
            .session_id
            .lock()
            .await
            .clone()
            .ok_or_else(acp::Error::method_not_found)?;
        let request = acp::SetSessionModeRequest::new(session_id, mode_id);
        acp::Agent::set_session_mode(&*agent, request).await?;
        Ok(())
    }

    /// Close this route — kill agent, drain queue, clear all state.
    /// Also kills any preview dev-servers registered with this session's ID.
    pub async fn close(&self, reason: Option<String>) {
        // Kill preview sessions owned by this session before resetting.
        if let Some(sid) = self.session_id.lock().await.clone() {
            crate::previews::kill_by_session(&sid.to_string());
        }
        self.full_reset().await;
        self.emit(SystemEvent::RouteClosed {
            route: self.route.clone(),
            reason,
        });
    }

    /// Switch agent kind — kill current agent, next prompt spawns a new one.
    pub async fn switch_agent(&self, agent_kind: String) -> anyhow::Result<String> {
        let agent_kind =
            crate::resources::resolve_agent_id(&agent_kind).map_err(anyhow::Error::msg)?;
        tracing::info!(
            "[Conversation] switch_agent route={} new_kind={}",
            self.route, agent_kind
        );
        self.full_reset().await;
        *self.cli_kind.lock().await = Some(agent_kind.clone());
        let _ = self.change_tx.send(());
        tracing::info!(
            "[Conversation] switch_agent done route={} cli_kind={:?}",
            self.route, agent_kind
        );
        Ok(agent_kind)
    }

    /// Switch profile — kill current agent, next prompt spawns a new one.
    pub async fn switch_profile(&self, profile: String) {
        tracing::info!(
            "[Conversation] switch_profile route={} new_profile={}",
            self.route, profile
        );
        self.full_reset().await;
        *self.profile.lock().await = Some(profile);
        let _ = self.change_tx.send(());
    }

    /// Reset session — kill session but keep agent (start a fresh thread).
    pub async fn reset_session(&self) {
        *self.session_id.lock().await = None;
        let _ = self.change_tx.send(());
    }

    /// Update cached agent commands (called when `available_commands_update` arrives).
    pub async fn update_agent_commands(&self, commands: serde_json::Value) {
        *self.agent_commands.lock().await = commands;
    }

    /// Get the cached list of available agent commands.
    pub async fn list_agent_commands(&self) -> serde_json::Value {
        self.agent_commands.lock().await.clone()
    }

    /// Read the conversation's mutable runtime fields as a consistent-enough
    /// snapshot. Immutable fields (`route`, `started_at`, `bot_identity`)
    /// are exposed directly via their own getters; this method only covers
    /// the fields that can change after construction.
    ///
    /// Internally takes several short mutex locks in sequence; since
    /// mutations typically batch (e.g. `ensure_agent` updates
    /// cli_kind/profile/initialize/failed together under the same async
    /// call chain), callers at dashboard polling cadence see a coherent
    /// view in practice.
    pub async fn state(&self) -> ConversationState {
        ConversationState {
            cli_kind: self.cli_kind.lock().await.clone(),
            profile: self.profile.lock().await.clone(),
            session_id: self.session_id.lock().await.clone(),
            workspace: self.workspace.lock().await.clone(),
            busy: *self.busy.lock().await,
            failed: self.failed.lock().await.clone(),
            initialize: self.initialize.lock().await.clone(),
        }
    }

    /// Immutable fields exposed as direct getters. `route` is already a
    /// `pub` field and callers should read it directly.
    pub fn started_at(&self) -> u64 { self.started_at }
    pub fn bot_identity(&self) -> Option<&str> { self.bot_identity.as_deref() }

    // Agent + session lifecycle (ensure_agent, ensure_session, full_reset)
    // lives in `lifecycle.rs`.

    // -----------------------------------------------------------------------
    // Event emission
    // -----------------------------------------------------------------------

    fn emit(&self, event: SystemEvent) {
        let _ = self.event_tx.send(event);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
