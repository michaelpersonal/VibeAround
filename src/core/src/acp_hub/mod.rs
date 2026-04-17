//! ACPHub: conversation management center.
//!
//! Manages per-route ACPPods, calls acp::Agent methods directly on bridges
//! (no command/event enum intermediaries). Emits SystemEvents for dashboard.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::broadcast;

use crate::acp::routing::RouteKey;
use crate::agent_factory::runtime::BridgeClientHandler;

use agent_client_protocol as acp;

use self::pod::ACPPod;

pub mod event;
pub mod pod;

pub use event::SystemEvent;
pub use pod::PodSnapshot;

pub struct ACPHub {
    pods: DashMap<RouteKey, Arc<ACPPod>>,
    event_tx: broadcast::Sender<SystemEvent>,
}

impl ACPHub {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            pods: DashMap::new(),
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SystemEvent> {
        self.event_tx.subscribe()
    }

    // -----------------------------------------------------------------------
    // Conversation lifecycle — direct methods, no command enums
    // -----------------------------------------------------------------------

    /// Send a prompt on a route. Handles bridge init and session creation
    /// transparently on first call.
    pub async fn prompt(
        &self,
        route: RouteKey,
        cli_kind: Option<String>,
        content_blocks: Vec<acp::ContentBlock>,
        handler: Arc<dyn BridgeClientHandler>,
    ) -> acp::Result<acp::PromptResponse> {
        let pod = self.get_or_create_pod(route);
        pod.prompt(cli_kind, content_blocks, handler).await
    }

    /// Cancel the active turn on a route.
    pub async fn cancel(&self, route: &RouteKey) -> acp::Result<()> {
        let Some(pod) = self.get_pod(route) else {
            return Err(acp::Error::method_not_found());
        };
        pod.cancel().await
    }

    /// Close a route — kill bridge, remove pod.
    pub async fn close(&self, route: &RouteKey, reason: Option<String>) {
        if let Some((_, pod)) = self.pods.remove(route) {
            pod.close(reason).await;
        }
    }

    /// Switch agent kind on a route (creates pod if needed).
    pub async fn switch_agent(&self, route: &RouteKey, agent_kind: String) {
        let pod = self.get_or_create_pod(route.clone());
        pod.switch_agent(agent_kind).await;
    }

    /// Switch profile on a route (creates pod if needed).
    pub async fn switch_profile(&self, route: &RouteKey, profile: String) {
        let pod = self.get_or_create_pod(route.clone());
        pod.switch_profile(profile).await;
    }

    /// Reset session on a route (new conversation, same agent).
    pub async fn reset_session(&self, route: &RouteKey) {
        if let Some(pod) = self.get_pod(route) {
            pod.reset_session().await;
        }
    }

    /// Set the permission mode of the current session on a route.
    /// Returns an error if there is no active session yet.
    pub async fn set_session_mode(
        &self,
        route: &RouteKey,
        mode_id: String,
    ) -> acp::Result<()> {
        let pod = self
            .get_pod(route)
            .ok_or_else(acp::Error::method_not_found)?;
        pod.set_session_mode(mode_id).await
    }

    /// Get a snapshot of a route's pod state.
    pub async fn snapshot(&self, route: &RouteKey) -> Option<PodSnapshot> {
        let pod = self.get_pod(route)?;
        Some(pod.snapshot().await)
    }

    /// Get cached available agent commands for a route.
    pub async fn list_agent_commands(&self, route: &RouteKey) -> serde_json::Value {
        match self.get_pod(route) {
            Some(pod) => pod.list_agent_commands().await,
            None => serde_json::Value::Array(vec![]),
        }
    }

    /// Update cached agent commands for a route (called on available_commands_update).
    pub async fn list_agent_commands_update(&self, route: &RouteKey, commands: serde_json::Value) {
        if let Some(pod) = self.get_pod(route) {
            pod.update_agent_commands(commands).await;
        }
    }

    /// Shutdown all routes.
    pub async fn shutdown_all(&self) {
        let routes: Vec<RouteKey> = self.pods.iter().map(|e| e.key().clone()).collect();
        for route in routes {
            self.close(&route, Some("shutdown".to_string())).await;
        }
    }

    // -----------------------------------------------------------------------
    // Session handover — pod lifecycle only
    // -----------------------------------------------------------------------

    /// Prepare a pod on a route for session pickup — set the resume_session_id
    /// and cwd so the next prompt spawns a bridge that loads the given session.
    pub async fn prepare_pickup(
        &self,
        route: RouteKey,
        cli_kind: String,
        resume_session_id: String,
        cwd: Option<String>,
    ) {
        let pod = self.get_or_create_pod(route);
        pod.set_handover(cli_kind, resume_session_id, cwd).await;
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    fn get_pod(&self, route: &RouteKey) -> Option<Arc<ACPPod>> {
        self.pods.get(route).map(|e| Arc::clone(&e))
    }

    fn get_or_create_pod(&self, route: RouteKey) -> Arc<ACPPod> {
        if let Some(existing) = self.get_pod(&route) {
            return existing;
        }

        let pod = Arc::new(ACPPod::new(route.clone(), self.event_tx.clone()));
        self.pods.insert(route.clone(), Arc::clone(&pod));
        let _ = self
            .event_tx
            .send(SystemEvent::RouteCreated { route });
        pod
    }
}
