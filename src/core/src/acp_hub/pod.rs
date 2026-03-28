//! ACPPod: per-route conversation state.
//!
//! Owns the agent bridge directly (no external cache). Calls acp::Agent
//! methods on the bridge without command enum intermediaries.

use std::collections::VecDeque;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::{broadcast, Mutex, Notify};

use crate::acp::routing::RouteKey;
use crate::agent_factory::runtime::{AcpBridge, BridgeClientHandler};
use crate::config;

use agent_client_protocol as acp;

use super::event::SystemEvent;

// ---------------------------------------------------------------------------
// PodSnapshot — serializable view of pod state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodSnapshot {
    pub route: RouteKey,
    pub bot_identity: Option<String>,
    pub session_id: Option<String>,
    pub cli_kind: Option<String>,
    pub profile: Option<String>,
    pub busy: bool,
    pub failed: Option<String>,
    pub started_at: u64,
    pub initialize: Option<acp::InitializeResponse>,
}

impl PodSnapshot {
    pub fn service_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.route.channel_kind,
            self.route.chat_id,
            self.profile.clone().unwrap_or_else(|| "default".to_string()),
            self.cli_kind.clone().unwrap_or_else(|| "unknown".to_string())
        )
    }
}

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
    initialize: Mutex<Option<acp::InitializeResponse>>,
    busy: Mutex<bool>,
    failed: Mutex<Option<String>>,
    started_at: u64,
    event_tx: broadcast::Sender<SystemEvent>,
    // Prompt queue — serialize concurrent prompts on the same route
    in_flight: Mutex<bool>,
    pending: Mutex<VecDeque<Arc<Notify>>>,
    /// Cached available commands from the agent's `available_commands_update` notification.
    /// Updated dynamically as the agent reports its command set.
    agent_commands: Mutex<serde_json::Value>,
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
            initialize: Mutex::new(None),
            busy: Mutex::new(false),
            failed: Mutex::new(None),
            started_at: unix_now_secs(),
            event_tx,
            in_flight: Mutex::new(false),
            pending: Mutex::new(VecDeque::new()),
            agent_commands: Mutex::new(serde_json::Value::Array(vec![])),
        }
    }

    // -----------------------------------------------------------------------
    // Public API — direct methods, no command enums
    // -----------------------------------------------------------------------

    /// Send a prompt to the agent. Handles bridge init and session creation
    /// transparently on first call.
    pub async fn prompt(
        self: &Arc<Self>,
        cli_kind: Option<String>,
        text: String,
        downstream_handler: Arc<dyn BridgeClientHandler>,
    ) -> acp::Result<acp::PromptResponse> {
        // Acquire turn slot — wait if another prompt is in-flight
        let wait_turn = {
            let mut in_flight = self.in_flight.lock().await;
            if !*in_flight {
                *in_flight = true;
                None
            } else {
                let notify = Arc::new(Notify::new());
                self.pending.lock().await.push_back(Arc::clone(&notify));
                Some(notify)
            }
        };
        if let Some(notify) = wait_turn {
            notify.notified().await;
        }

        eprintln!(
            "[ACPPod] prompt route={} cli_kind={:?} text_len={}",
            self.route,
            cli_kind,
            text.len()
        );

        // Mark busy
        *self.busy.lock().await = true;
        *self.failed.lock().await = None;
        self.emit_snapshot().await;

        let result: acp::Result<acp::PromptResponse> = async {
            let bridge = self
                .ensure_bridge(cli_kind, None, downstream_handler)
                .await
                .map_err(|error| {
                    eprintln!("[ACPPod] ensure_bridge failed route={}: {}", self.route, error);
                    acp::Error::internal_error()
                })?;

            let session_id = self.ensure_session(&bridge).await?;

            let request = acp::PromptRequest::new(
                session_id,
                vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
            );
            acp::Agent::prompt(&*bridge, request).await
        }
        .await;

        // Mark idle
        *self.busy.lock().await = false;
        if let Err(error) = &result {
            *self.failed.lock().await = Some(error.message.to_string());
        }
        self.emit_snapshot().await;

        // Advance queue — let next pending prompt proceed
        let next = self.pending.lock().await.pop_front();
        if let Some(next) = next {
            next.notify_one();
        } else {
            *self.in_flight.lock().await = false;
        }

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

    /// Close this route — kill bridge, drain queue, clear all state.
    pub async fn close(&self, reason: Option<String>) {
        self.full_reset().await;
        self.emit(SystemEvent::RouteClosed {
            route: self.route.clone(),
            reason,
        });
    }

    /// Switch agent kind — kill current bridge, drain queue, next prompt spawns new one.
    pub async fn switch_agent(&self, agent_kind: String) {
        eprintln!("[ACPPod] switch_agent route={} new_kind={}", self.route, agent_kind);
        self.full_reset().await;
        *self.cli_kind.lock().await = Some(agent_kind.clone());
        self.emit_snapshot().await;
        eprintln!("[ACPPod] switch_agent done route={} cli_kind={:?}", self.route, agent_kind);
    }

    /// Switch profile — kill current bridge, drain queue, next prompt spawns new one.
    pub async fn switch_profile(&self, profile: String) {
        eprintln!("[ACPPod] switch_profile route={} new_profile={}", self.route, profile);
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
        downstream_handler: Arc<dyn BridgeClientHandler>,
    ) -> Result<Arc<AcpBridge>, String> {
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
                eprintln!("[ACPPod] ensure_bridge reusing existing bridge route={}", self.route);
                return Ok(existing);
            }
        }

        // Resolve again after potential switch
        let cli_kind = self.cli_kind.lock().await.clone()
            .unwrap_or_else(|| config::ensure_loaded().default_agent.clone());
        eprintln!("[ACPPod] ensure_bridge spawning new bridge route={} kind={}", self.route, cli_kind);
        let profile = self
            .profile
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "default".to_string());

        // Wrap downstream handler with our observation hook
        let handler: Arc<dyn BridgeClientHandler> = Arc::new(SessionBridgeHandler {
            route: self.route.clone(),
            event_tx: self.event_tx.clone(),
            downstream: downstream_handler,
        });

        let ready = match crate::agent_factory::spawn_bridge(
            &self.route.channel_kind,
            &cli_kind,
            resume_session_id.clone(),
            handler,
        )
        .await
        {
            Ok(ready) => ready,
            Err(error) => {
                *self.failed.lock().await = Some(error.clone());
                self.emit(SystemEvent::AgentInitializeFailed {
                    route: self.route.clone(),
                    cli_kind: Some(cli_kind),
                    error: error.clone(),
                });
                self.emit_snapshot().await;
                return Err(error);
            }
        };

        // Store bridge and metadata
        eprintln!(
            "[ACPPod] bridge ready route={} kind={} agent_info={:?}",
            self.route,
            cli_kind,
            ready.initialize.agent_info
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

        let workspace = config::data_dir().join("workspaces");
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

    /// Kill the current bridge and clear related state.
    /// Full reset: kill bridge, drain queue, clear all state.
    /// Used by switch_agent/switch_profile to ensure clean slate.
    async fn full_reset(&self) {
        // Kill bridge
        if let Some(bridge) = self.bridge.lock().await.take() {
            bridge.shutdown().await;
            eprintln!("[ACPPod] full_reset killed bridge route={}", self.route);
        }
        // Drain queue — wake all pending prompts (they'll fail on missing bridge)
        {
            let mut pending = self.pending.lock().await;
            while let Some(notify) = pending.pop_front() {
                notify.notify_one();
            }
        }
        *self.in_flight.lock().await = false;
        *self.session_id.lock().await = None;
        *self.initialize.lock().await = None;
        *self.failed.lock().await = None;
        *self.busy.lock().await = false;
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
// SessionBridgeHandler — ACPHub's observation hook on the bridge
// ---------------------------------------------------------------------------

struct SessionBridgeHandler {
    route: RouteKey,
    event_tx: broadcast::Sender<SystemEvent>,
    downstream: Arc<dyn BridgeClientHandler>,
}

#[async_trait::async_trait(?Send)]
impl BridgeClientHandler for SessionBridgeHandler {
    async fn session_notification(&self, args: acp::SessionNotification) -> acp::Result<()> {
        // TODO: capture for chat history here

        // Forward to channel handler
        self.downstream.session_notification(args).await
    }

    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        self.downstream.request_permission(args).await
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
