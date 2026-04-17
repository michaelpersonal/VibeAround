//! `ACPPod` — per-route conversation state.
//!
//! Owns the agent bridge directly (no external cache). Calls `acp::Agent`
//! methods on the bridge without command enum intermediaries.
//!
//! ## Module layout
//!
//! - [`snapshot`]        — `PodSnapshot` (serialized view of pod state).
//! - [`media`]           — relocate cached media from staging to the
//!                         session-scoped workspace path before each prompt.
//! - [`bridge_handler`]  — `SessionBridgeHandler` wrapper that suppresses
//!                         session-notification replay during handover
//!                         `load_session` to keep history out of the IM feed.

mod bridge_handler;
mod media;
mod snapshot;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use tokio::sync::{broadcast, Mutex};

use agent_client_protocol as acp;

use crate::acp::routing::RouteKey;
use crate::agent_factory::runtime::{AcpBridge, BridgeClientHandler};
use crate::config;

use super::event::SystemEvent;

use bridge_handler::SessionBridgeHandler;
use media::relocate_cached_media;

pub use snapshot::PodSnapshot;

// ---------------------------------------------------------------------------
// ACPPod
// ---------------------------------------------------------------------------

pub struct ACPPod {
    pub route: RouteKey,
    bot_identity: Option<String>,
    bridge: Mutex<Option<Arc<AcpBridge>>>,
    session_id: Mutex<Option<String>>,
    cli_kind: Mutex<Option<String>>,
    profile: Mutex<Option<String>>,
    /// Resolved workspace path, set when bridge is spawned.
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
    /// Released just before the first prompt is sent (not when bridge is ready),
    /// because some agents (Gemini) continue replaying after load_session returns.
    suppress_replay: Mutex<Option<Arc<AtomicBool>>>,
}

impl ACPPod {
    pub fn new(route: RouteKey, event_tx: broadcast::Sender<SystemEvent>) -> Self {
        Self {
            route,
            bot_identity: None,
            bridge: Mutex::new(None),
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
        }
    }

    /// Prepare this pod for a session pickup. Sets cli_kind, resume_session_id,
    /// and optionally cwd so the next prompt spawns a bridge that resumes the
    /// given session in the correct workspace.
    pub async fn set_handover(
        &self,
        cli_kind: String,
        resume_session_id: String,
        cwd: Option<String>,
    ) {
        self.full_reset().await;
        *self.cli_kind.lock().await = Some(cli_kind);
        *self.handover_resume_session_id.lock().await = Some(resume_session_id);
        *self.handover_cwd.lock().await = cwd;
    }

    // -----------------------------------------------------------------------
    // Public API — direct methods, no command enums
    // -----------------------------------------------------------------------

    /// Send a prompt to the agent. Handles bridge init and session creation
    /// transparently on first call.
    pub async fn prompt(
        self: &Arc<Self>,
        cli_kind: Option<String>,
        content_blocks: Vec<acp::ContentBlock>,
        downstream_handler: Arc<dyn BridgeClientHandler>,
    ) -> acp::Result<acp::PromptResponse> {
        // No prompt_lock — prompts are forwarded to the agent immediately.
        // CLI agents (Claude Code, Codex, Gemini CLI) accept input at any
        // time and queue/interrupt internally via ACP. Blocking here caused
        // user-visible hangs when a turn didn't end (e.g. background tasks).
        eprintln!(
            "[ACPPod] prompt route={} cli_kind={:?} blocks={}",
            self.route,
            cli_kind,
            content_blocks.len()
        );

        *self.busy.lock().await = true;
        *self.failed.lock().await = None;
        self.emit_snapshot().await;

        let result: acp::Result<acp::PromptResponse> = async {
            // Take handover state (consumed once)
            let resume_sid = self.handover_resume_session_id.lock().await.take();
            let resume_cwd = self.handover_cwd.lock().await.take();

            let bridge = self
                .ensure_bridge(cli_kind, resume_sid, resume_cwd, downstream_handler)
                .await
                .map_err(|error| {
                    eprintln!(
                        "[ACPPod] ensure_bridge failed route={}: {:#}",
                        self.route, error
                    );
                    acp::Error::new(-32603, error.to_string())
                })?;

            let session_id = self.ensure_session(&bridge).await?;

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

            eprintln!(
                "[ACPPod] prompt SENDING route={} session={}",
                self.route, session_id
            );
            let request = acp::PromptRequest::new(session_id, content_blocks);
            let response = acp::Agent::prompt(&*bridge, request).await;
            eprintln!(
                "[ACPPod] prompt RETURNED route={} ok={}",
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
        self.emit_snapshot().await;

        result
    }

    /// Cancel the active turn.
    pub async fn cancel(&self) -> acp::Result<()> {
        let bridge = self
            .bridge
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
        acp::Agent::cancel(&*bridge, acp::CancelNotification::new(session_id)).await
    }

    /// Switch the current session's permission mode. Requires an active
    /// bridge + session (caller should ensure this — no auto-spawn because
    /// the mode is a session property that only exists after initialization).
    pub async fn set_session_mode(&self, mode_id: String) -> acp::Result<()> {
        let bridge = self
            .bridge
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
        acp::Agent::set_session_mode(&*bridge, request).await?;
        Ok(())
    }

    /// Close this route — kill bridge, drain queue, clear all state.
    /// Also kills any preview dev-servers registered with this session's ID.
    pub async fn close(&self, reason: Option<String>) {
        // Kill preview sessions owned by this agent session before resetting.
        if let Some(sid) = self.session_id.lock().await.clone() {
            crate::preview_entries::kill_by_session(&sid.to_string());
        }
        self.full_reset().await;
        self.emit(SystemEvent::RouteClosed {
            route: self.route.clone(),
            reason,
        });
    }

    /// Switch agent kind — kill current bridge, drain queue, next prompt spawns new one.
    pub async fn switch_agent(&self, agent_kind: String) {
        eprintln!(
            "[ACPPod] switch_agent route={} new_kind={}",
            self.route, agent_kind
        );
        self.full_reset().await;
        *self.cli_kind.lock().await = Some(agent_kind.clone());
        self.emit_snapshot().await;
        eprintln!(
            "[ACPPod] switch_agent done route={} cli_kind={:?}",
            self.route, agent_kind
        );
    }

    /// Switch profile — kill current bridge, drain queue, next prompt spawns new one.
    pub async fn switch_profile(&self, profile: String) {
        eprintln!(
            "[ACPPod] switch_profile route={} new_profile={}",
            self.route, profile
        );
        self.full_reset().await;
        *self.profile.lock().await = Some(profile);
        self.emit_snapshot().await;
    }

    /// Reset session — kill session but keep bridge (start fresh conversation).
    pub async fn reset_session(&self) {
        *self.session_id.lock().await = None;
        self.emit_snapshot().await;
    }

    /// Update cached agent commands (called when `available_commands_update` arrives).
    pub async fn update_agent_commands(&self, commands: serde_json::Value) {
        *self.agent_commands.lock().await = commands;
    }

    /// Get the cached list of available agent commands.
    pub async fn list_agent_commands(&self) -> serde_json::Value {
        self.agent_commands.lock().await.clone()
    }

    /// Get a serializable snapshot of pod state.
    pub async fn snapshot(&self) -> PodSnapshot {
        PodSnapshot {
            route: self.route.clone(),
            bot_identity: self.bot_identity.clone(),
            session_id: self.session_id.lock().await.clone(),
            cli_kind: self.cli_kind.lock().await.clone(),
            profile: self.profile.lock().await.clone(),
            workspace: self.workspace.lock().await.clone(),
            busy: *self.busy.lock().await,
            failed: self.failed.lock().await.clone(),
            started_at: self.started_at,
            initialize: self.initialize.lock().await.clone(),
        }
    }

    // -----------------------------------------------------------------------
    // Internal — bridge and session lifecycle
    // -----------------------------------------------------------------------

    /// Ensure a bridge exists, spawning one via agent_factory if needed.
    async fn ensure_bridge(
        self: &Arc<Self>,
        cli_kind: Option<String>,
        resume_session_id: Option<String>,
        resume_cwd: Option<String>,
        downstream_handler: Arc<dyn BridgeClientHandler>,
    ) -> anyhow::Result<Arc<AcpBridge>> {
        // Resolve which agent kind to use
        let stored_cli_kind = self.cli_kind.lock().await.clone();
        let resolved_cli_kind = stored_cli_kind
            .clone()
            .or(cli_kind.clone())
            .unwrap_or_else(|| config::ensure_loaded().default_agent.clone());

        // If bridge exists, check if caller requested a different agent (implicit switch)
        if let Some(existing) = self.bridge.lock().await.clone() {
            let needs_switch = cli_kind
                .as_ref()
                .map(|requested| {
                    stored_cli_kind
                        .as_ref()
                        .map(|stored| stored != requested)
                        .unwrap_or(false)
                })
                .unwrap_or(false);

            if needs_switch {
                let new_kind = cli_kind.unwrap();
                eprintln!(
                    "[ACPPod] ensure_bridge implicit switch route={} {} → {}",
                    self.route, resolved_cli_kind, new_kind
                );
                self.full_reset().await;
                *self.cli_kind.lock().await = Some(new_kind.clone());
                // Fall through to spawn new bridge below
            } else {
                eprintln!(
                    "[ACPPod] ensure_bridge reusing existing bridge route={}",
                    self.route
                );
                return Ok(existing);
            }
        }

        // Resolve again after potential switch
        let cli_kind = self
            .cli_kind
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| config::ensure_loaded().default_agent.clone());
        eprintln!(
            "[ACPPod] ensure_bridge spawning new bridge route={} kind={}",
            self.route, cli_kind
        );
        let profile = self
            .profile
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "default".to_string());

        // Resolve workspace — handover must include cwd, normal prompt uses default
        let is_handover = resume_session_id.is_some();
        let workspace = match resume_cwd {
            Some(cwd) => std::path::PathBuf::from(cwd),
            None if is_handover => {
                return Err(anyhow!(
                    "Session pickup is missing the working directory. \
                     Please re-run the handover to get an updated /pickup command that includes the cwd."
                ));
            }
            None => config::ensure_loaded().resolve_workspace(&cli_kind),
        };

        // Track workspace for snapshot (used by /handover Direction 2)
        *self.workspace.lock().await = Some(workspace.to_string_lossy().to_string());

        // Wrap downstream handler — suppress replay during handover load_session
        let is_handover = resume_session_id.is_some();
        let suppress_replay = Arc::new(AtomicBool::new(is_handover));
        let handler: Arc<dyn BridgeClientHandler> = Arc::new(SessionBridgeHandler {
            downstream: downstream_handler,
            suppress_replay: Arc::clone(&suppress_replay),
        });

        let ready = match crate::agent_factory::spawn_bridge(
            &self.route.channel_kind,
            &self.route.chat_id,
            &cli_kind,
            &workspace,
            resume_session_id.clone(),
            handler,
        )
        .await
        {
            Ok(ready) => ready,
            Err(error) => {
                let msg = error.to_string();
                *self.failed.lock().await = Some(msg.clone());
                self.emit(SystemEvent::AgentInitializeFailed {
                    route: self.route.clone(),
                    cli_kind: Some(cli_kind),
                    error: msg,
                });
                self.emit_snapshot().await;
                return Err(error);
            }
        };

        // Store suppress_replay on the pod — released before the first prompt,
        // not here, because some agents (Gemini) continue replaying after load_session.
        if is_handover {
            *self.suppress_replay.lock().await = Some(suppress_replay);
        }

        // Store bridge and metadata
        eprintln!(
            "[ACPPod] bridge ready route={} kind={} agent_info={:?}",
            self.route, cli_kind, ready.initialize.agent_info
        );
        *self.bridge.lock().await = Some(Arc::clone(&ready.bridge));
        *self.cli_kind.lock().await = Some(cli_kind.clone());
        *self.profile.lock().await = Some(profile.clone());
        *self.initialize.lock().await = Some(ready.initialize.clone());
        *self.failed.lock().await = None;

        if let Some(session_id) = resume_session_id.or(ready.startup_session_id) {
            *self.session_id.lock().await = Some(session_id.clone());
            self.emit(SystemEvent::SessionReady {
                route: self.route.clone(),
                session_id,
            });
        }

        self.spawn_provider_session_watcher(&ready.bridge).await;
        self.emit(SystemEvent::AgentInitialized {
            route: self.route.clone(),
            cli_kind: Some(cli_kind),
            profile: Some(profile),
            initialize: ready.initialize.clone(),
        });
        self.emit_snapshot().await;

        Ok(ready.bridge)
    }

    /// Ensure a session exists, creating one if needed.
    async fn ensure_session(&self, bridge: &Arc<AcpBridge>) -> acp::Result<String> {
        if let Some(session_id) = self.session_id.lock().await.clone() {
            return Ok(session_id);
        }

        let agent_kind = self
            .cli_kind
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "claude".to_string());
        let workspace = config::ensure_loaded().resolve_workspace(&agent_kind);
        let response =
            acp::Agent::new_session(&**bridge, acp::NewSessionRequest::new(workspace)).await?;
        let session_id = response.session_id.to_string();
        *self.session_id.lock().await = Some(session_id.clone());

        self.emit(SystemEvent::SessionReady {
            route: self.route.clone(),
            session_id: session_id.clone(),
        });
        self.emit_snapshot().await;

        Ok(session_id)
    }

    /// Full reset: kill bridge and clear all state.
    ///
    /// Does not wait for any in-flight prompt — the bridge shutdown signal is
    /// sent immediately. Any concurrent `acp::Agent::prompt` future will
    /// receive an ACP error. Subsequent prompts will re-spawn a fresh bridge
    /// via `ensure_bridge`.
    async fn full_reset(&self) {
        if let Some(bridge) = self.bridge.lock().await.take() {
            bridge.shutdown().await;
            eprintln!("[ACPPod] full_reset killed bridge route={}", self.route);
        }
        *self.session_id.lock().await = None;
        *self.initialize.lock().await = None;
        *self.failed.lock().await = None;
        *self.busy.lock().await = false;
        *self.handover_resume_session_id.lock().await = None;
        *self.handover_cwd.lock().await = None;
        *self.suppress_replay.lock().await = None;
        eprintln!("[ACPPod] full_reset done route={}", self.route);
    }

    async fn spawn_provider_session_watcher(self: &Arc<Self>, bridge: &Arc<AcpBridge>) {
        let Some(mut rx) = bridge.take_provider_session_id_rx().await else {
            return;
        };
        let pod = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(session_id) = rx.recv().await {
                let Some(pod) = pod.upgrade() else {
                    break;
                };
                *pod.session_id.lock().await = Some(session_id);
                pod.emit_snapshot().await;
            }
        });
    }

    // -----------------------------------------------------------------------
    // Event emission
    // -----------------------------------------------------------------------

    fn emit(&self, event: SystemEvent) {
        let _ = self.event_tx.send(event);
    }

    async fn emit_snapshot(&self) {
        self.emit(SystemEvent::SnapshotChanged {
            route: self.route.clone(),
            snapshot: self.snapshot().await,
        });
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
