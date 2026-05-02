//! `Agent` — one live ACP connection to a coding CLI process.
//!
//! Each `Agent` wraps a single ACP `ClientSideConnection` to a real agent
//! subprocess. Northbound it implements `acp::Agent` so callers
//! ([`Conversation`]) use standard ACP methods directly. Southbound
//! `Client` events (`session_notification`, `request_permission`) are
//! forwarded to a caller-supplied [`AgentClientHandler`].
//!
//! Only stdio ACP is supported — no provider trait, no pluggable
//! transport. If another transport is ever needed, reintroduce a trait
//! at that time.
//!
//! ## Lifecycle
//!
//! Spawn and supervision are delegated to [`process::Supervisor`]. The
//! agent's [`RestartPolicy`] is `Never` — crashes surface via the normal
//! supervisor broadcast and it's the owning [`Conversation`]'s decision
//! whether to re-spawn. `Agent::shutdown` translates to
//! `supervisor.force_stop(process_id)`.
//!
//! [`Conversation`]: crate::conversations::Conversation
//! [`process::Supervisor`]: crate::process::Supervisor
//! [`RestartPolicy`]: crate::process::RestartPolicy

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Context};
use tokio::sync::{oneshot, Mutex};

use agent_client_protocol as acp;

use crate::process::bridge::{BridgeFactory, ProcessBridge};
use crate::process::registry::ProcessKind;
use crate::process::supervisor::{ProcessId, RestartPolicy, SpawnSpec, Supervisor};
use crate::routing::RouteKey;

use super::bridge::AcpAgentBridge;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Callback trait for southbound ACP client events forwarded to the caller.
///
/// The agent calls these methods when the real CLI sends notifications or
/// requests back through the ACP client channel.
#[async_trait::async_trait(?Send)]
pub trait AgentClientHandler: Send + Sync + 'static {
    async fn session_notification(
        &self,
        args: acp::SessionNotification,
    ) -> acp::Result<()>;

    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse>;
}

/// Handle returned from a successful [`Agent::spawn`].
pub struct AgentReady {
    pub agent: Arc<Agent>,
    pub startup_session_id: Option<String>,
    pub initialize: acp::InitializeResponse,
}

/// One live ACP-speaking coding CLI. Northbound: `acp::Agent`. Southbound:
/// delegates to an [`AgentClientHandler`].
pub struct Agent {
    /// The southbound ACP connection to the real agent process.
    conn: acp::ClientSideConnection,
    agent_id: String,
    /// ACP initialize response from first startup.
    initialize: acp::InitializeResponse,
    /// ACP session ID obtained from new_session / load_session.
    session_id: Mutex<Option<String>>,
    /// Supervisor handle installed by [`Agent::spawn`] after registration.
    /// `None` until the registration returns — effectively
    /// a moment-of-initialization gap where `shutdown()` is a no-op.
    process_id: OnceLock<ProcessId>,
}

impl Agent {
    /// Spawn a new agent through the process supervisor.
    ///
    /// `agent_id` must match an entry in `resources/agents.json`. The
    /// binary is lazily installed on first miss (npm or `install_cmd`).
    ///
    /// `route` is used to build a supervisor label unique to the owning
    /// conversation (`<agent_id>:<channel_kind>:<chat_id>`) so that
    /// multiple concurrent agents of the same kind remain distinguishable
    /// in snapshots and logs.
    pub async fn spawn(
        agent_id: String,
        route: &RouteKey,
        workspace: &Path,
        resume_session_id: Option<String>,
        client_handler: Arc<dyn AgentClientHandler>,
        extra_env: Vec<(String, String)>,
    ) -> anyhow::Result<AgentReady> {
        let cwd = workspace.to_path_buf();
        let label = format!("{}:{}", agent_id, route);

        // Resolve program + args + install if needed.
        let (program, resolved_args) = resolve_agent_program(&agent_id).await?;
        tracing::info!(
            "[{}] spawning {} {} in {:?}",
            label, program, resolved_args.join(" "), cwd
        );

        let mut spec = SpawnSpec::new(program)
            .args(resolved_args)
            .cwd(cwd.clone());
        for (k, v) in extra_env {
            spec = spec.env(k, v);
        }

        // The supervisor's factory is `Fn()`, so one-shot state (the
        // ready sender + client handler) has to go through a Mutex<Option>
        // that the first (and only) invocation drains. RestartPolicy::Never
        // guarantees there is no second invocation.
        let (ready_tx, ready_rx) = oneshot::channel::<anyhow::Result<AgentReady>>();
        let bridge = AcpAgentBridge {
            agent_id: agent_id.clone(),
            cwd,
            resume_session_id,
            client_handler,
            ready_tx,
        };
        let slot: Arc<parking_lot::Mutex<Option<AcpAgentBridge>>> =
            Arc::new(parking_lot::Mutex::new(Some(bridge)));
        let factory: BridgeFactory = Box::new(move || {
            let b = slot.lock().take().expect(
                "AcpAgentBridge factory called more than once — RestartPolicy::Never guarantees single-spawn",
            );
            Box::new(b) as Box<dyn ProcessBridge>
        });

        let id = Supervisor::global().register(
            ProcessKind::AcpAgent,
            label,
            spec,
            RestartPolicy::Never,
            factory,
        );

        let err_label = agent_id.clone();
        let ready = ready_rx
            .await
            .map_err(|_| anyhow!("Agent bridge for {} died during init", err_label))??;

        // Stash the id back onto the Agent so shutdown() can reach it.
        let _ = ready.agent.process_id.set(id);

        Ok(ready)
    }

    /// Constructor used by the bridge once the ACP handshake has succeeded.
    /// Not `pub` externally — only `agent::bridge` needs it.
    pub(crate) fn from_connection(
        conn: acp::ClientSideConnection,
        agent_id: String,
        initialize: acp::InitializeResponse,
        startup_session_id: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            conn,
            agent_id,
            initialize,
            session_id: Mutex::new(startup_session_id),
            process_id: OnceLock::new(),
        })
    }

    pub fn id(&self) -> &str {
        &self.agent_id
    }

    pub fn initialize_response(&self) -> acp::InitializeResponse {
        self.initialize.clone()
    }

    pub async fn session_id(&self) -> Option<String> {
        self.session_id.lock().await.clone()
    }

    /// Signal the supervisor to stop the agent process. No-op if the
    /// supervisor registration hasn't completed yet (extremely short
    /// window during `spawn`).
    pub async fn shutdown(&self) {
        tracing::info!("[{}-agent] shutdown signaled", self.agent_id);
        if let Some(id) = self.process_id.get() {
            if let Err(e) = Supervisor::global().force_stop(*id).await {
                tracing::info!(
                    "[{}-agent] supervisor force_stop failed: {}",
                    self.agent_id, e
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Northbound: implement acp::Agent so callers use standard ACP methods
// ---------------------------------------------------------------------------

#[async_trait::async_trait(?Send)]
impl acp::Agent for Agent {
    async fn initialize(&self, args: acp::InitializeRequest) -> acp::Result<acp::InitializeResponse> {
        self.conn.initialize(args).await
    }

    async fn authenticate(&self, args: acp::AuthenticateRequest) -> acp::Result<acp::AuthenticateResponse> {
        self.conn.authenticate(args).await
    }

    async fn new_session(&self, args: acp::NewSessionRequest) -> acp::Result<acp::NewSessionResponse> {
        let resp = self.conn.new_session(args).await?;
        *self.session_id.lock().await = Some(resp.session_id.to_string());
        Ok(resp)
    }

    async fn load_session(&self, args: acp::LoadSessionRequest) -> acp::Result<acp::LoadSessionResponse> {
        let session_id = args.session_id.clone();
        let resp = self.conn.load_session(args).await?;
        *self.session_id.lock().await = Some(session_id.to_string());
        Ok(resp)
    }

    async fn set_session_mode(&self, args: acp::SetSessionModeRequest) -> acp::Result<acp::SetSessionModeResponse> {
        self.conn.set_session_mode(args).await
    }

    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        self.conn.prompt(args).await
    }

    async fn cancel(&self, args: acp::CancelNotification) -> acp::Result<()> {
        self.conn.cancel(args).await
    }

    async fn set_session_config_option(
        &self,
        args: acp::SetSessionConfigOptionRequest,
    ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
        self.conn.set_session_config_option(args).await
    }

    async fn ext_method(&self, args: acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        self.conn.ext_method(args).await
    }

    async fn ext_notification(&self, args: acp::ExtNotification) -> acp::Result<()> {
        self.conn.ext_notification(args).await
    }
}

// ---------------------------------------------------------------------------
// Southbound: ACP Client handler that forwards to AgentClientHandler
// ---------------------------------------------------------------------------

pub(crate) struct AgentClient {
    handler: Arc<dyn AgentClientHandler>,
}

impl AgentClient {
    pub(crate) fn new(handler: Arc<dyn AgentClientHandler>) -> Self {
        Self { handler }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Client for AgentClient {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        self.handler.request_permission(args).await
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> acp::Result<()> {
        self.handler.session_notification(args).await
    }
}

// ---------------------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------------------

/// Resolve the agent's launch command, lazily installing the binary on
/// first miss. Returns `(program, args)` ready for a [`SpawnSpec`].
async fn resolve_agent_program(agent_id: &str) -> anyhow::Result<(String, Vec<String>)> {
    let agent_def = crate::resources::agent_by_id(agent_id)
        .ok_or_else(|| anyhow!("No resource definition for agent '{}'", agent_id))?;

    // 1. npm-based agents → `node <resolved_entry>`
    // 2. binary-download agents → install via install_cmd, run from PATH
    // 3. native agents → program + args from PATH
    if let Some(npm_pkg) = &agent_def.acp.npm_package {
        let bin_name = agent_def.acp.bin_name.as_deref().unwrap_or(npm_pkg);
        if crate::process::env::resolve_acp_agent_bin(bin_name).is_err() {
            tracing::info!("[{}-agent] auto-installing {} ...", agent_id, npm_pkg);
            super::install::auto_install_npm_agent(npm_pkg).await?;
        }
        let entry = crate::process::env::resolve_acp_agent_bin(bin_name)
            .with_context(|| format!("Resolving ACP agent '{}' (npm: {})", agent_id, npm_pkg))?;
        Ok((
            "node".to_string(),
            vec![entry.to_string_lossy().to_string()],
        ))
    } else if let Some(install_cmd) = &agent_def.acp.install_cmd {
        if !super::install::is_program_available(&agent_def.acp.program) {
            tracing::info!("[{}-agent] auto-installing via install cmd ...", agent_id);
            super::install::auto_install_agent_cmd(install_cmd, agent_id).await?;
        }
        Ok((agent_def.acp.program.clone(), agent_def.acp.args.clone()))
    } else {
        Ok((agent_def.acp.program.clone(), agent_def.acp.args.clone()))
    }
}

