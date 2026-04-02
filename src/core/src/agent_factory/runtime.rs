//! AcpBridge: flat ACP bridge — northbound Agent + southbound Client.
//!
//! Each instance wraps a single `ClientSideConnection` to a real agent process.
//! The northbound surface implements `acp::Agent` so that ACPHub (or any
//! upstream caller) can call standard ACP methods directly.
//!
//! Southbound `Client` events (`session_notification`, `request_permission`)
//! are forwarded to a caller-supplied callback rather than being translated
//! into custom enums.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use tokio::sync::{mpsc, Mutex};

use super::provider::{AgentKind, AgentProvider};

use agent_client_protocol as acp;

/// Callback trait for southbound ACP client events forwarded to the caller.
///
/// The bridge calls these methods when the real agent sends notifications or
/// requests back through the ACP client channel.
#[async_trait::async_trait(?Send)]
pub trait BridgeClientHandler: Send + Sync + 'static {
    async fn session_notification(
        &self,
        args: acp::SessionNotification,
    ) -> acp::Result<()>;

    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse>;
}

pub struct BridgeReady {
    pub bridge: Arc<AcpBridge>,
    pub startup_session_id: Option<String>,
    pub initialize: acp::InitializeResponse,
}

/// A flat ACP bridge: one northbound Agent surface, one southbound Client connection.
pub struct AcpBridge {
    /// The southbound ACP connection to the real agent process.
    conn: acp::ClientSideConnection,
    kind: AgentKind,
    /// ACP initialize response from first bridge startup.
    initialize: acp::InitializeResponse,
    /// ACP session ID obtained from new_session / load_session.
    session_id: Mutex<Option<String>>,
    /// Provider session ID discovered out-of-band (e.g. Claude's session ID).
    provider_session_id_rx: Mutex<Option<mpsc::UnboundedReceiver<String>>>,
    /// Worker thread for the provider process (if any).
    _worker_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Cancellation token — signals the bridge thread to shut down.
    cancel_tx: tokio::sync::watch::Sender<bool>,
}

impl AcpBridge {
    /// Spawn a new bridge on a dedicated thread with its own tokio runtime + LocalSet.
    ///
    /// Returns bridge readiness info including ACP initialize response.
    /// The bridge is ready for ACP calls immediately after this returns.
    pub async fn spawn(
        provider: Arc<dyn AgentProvider>,
        kind: AgentKind,
        workspace: &Path,
        resume_session_id: Option<String>,
        client_handler: Arc<dyn BridgeClientHandler>,
    ) -> anyhow::Result<BridgeReady> {
        let cwd = workspace.to_path_buf();
        let (ready_tx, ready_rx) =
            tokio::sync::oneshot::channel::<anyhow::Result<BridgeReady>>();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

        std::thread::Builder::new()
            .name(format!("{}-bridge", kind))
            .spawn(move || {
                run_bridge_thread(
                    provider,
                    kind,
                    cwd,
                    ready_tx,
                    resume_session_id,
                    client_handler,
                    cancel_tx,
                    cancel_rx,
                );
            })
            .with_context(|| format!("Failed to spawn bridge thread for {}", kind))?;

        ready_rx
            .await
            .map_err(|_| anyhow!("Bridge thread for {} died during init", kind))?
    }

    pub fn kind(&self) -> AgentKind {
        self.kind
    }

    pub fn initialize_response(&self) -> acp::InitializeResponse {
        self.initialize.clone()
    }

    pub async fn session_id(&self) -> Option<String> {
        self.session_id.lock().await.clone()
    }

    /// Drain any out-of-band provider session IDs (e.g. Claude's session discovery).
    pub async fn take_provider_session_id_rx(&self) -> Option<mpsc::UnboundedReceiver<String>> {
        self.provider_session_id_rx.lock().await.take()
    }

    pub async fn shutdown(&self) {
        eprintln!("[{}-bridge] shutdown signaled", self.kind);
        let _ = self.cancel_tx.send(true);
    }
}

// --- Northbound: implement acp::Agent so callers use standard ACP methods ---

#[async_trait::async_trait(?Send)]
impl acp::Agent for AcpBridge {
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

// --- Southbound: ACP Client handler that forwards to BridgeClientHandler ---

struct BridgeClient {
    handler: Arc<dyn BridgeClientHandler>,
}

#[async_trait::async_trait(?Send)]
impl acp::Client for BridgeClient {
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

// --- Bridge thread ---

fn run_bridge_thread(
    provider: Arc<dyn AgentProvider>,
    kind: AgentKind,
    cwd: PathBuf,
    ready_tx: tokio::sync::oneshot::Sender<anyhow::Result<BridgeReady>>,
    resume_session_id: Option<String>,
    client_handler: Arc<dyn BridgeClientHandler>,
    cancel_tx: tokio::sync::watch::Sender<bool>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready_tx.send(Err(anyhow!("Failed to build bridge runtime: {}", e)));
            return;
        }
    };

    runtime.block_on(async move {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                match init_bridge(
                    provider, kind, cwd, resume_session_id, client_handler, cancel_tx,
                )
                .await
                {
                    Ok(ready) => {
                        let _ = ready_tx.send(Ok(ready));
                        // Wait for cancellation signal
                        let _ = cancel_rx.wait_for(|v| *v).await;
                        eprintln!("[{}-bridge] cancelled, exiting thread", kind);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .await;
    });
}

async fn init_bridge(
    provider: Arc<dyn AgentProvider>,
    kind: AgentKind,
    cwd: PathBuf,
    resume_session_id: Option<String>,
    client_handler: Arc<dyn BridgeClientHandler>,
    cancel_tx: tokio::sync::watch::Sender<bool>,
) -> anyhow::Result<BridgeReady> {
    use acp::Agent as _;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    let mut connection = provider.connect(&cwd).await?;
    let provider_session_id_rx = connection.session_id_rx.take();
    let worker_thread = connection.worker_thread.take();

    let (conn, handle_io) = acp::ClientSideConnection::new(
        BridgeClient {
            handler: client_handler,
        },
        connection.write_stream.compat_write(),
        connection.read_stream.compat(),
        |fut| {
            tokio::task::spawn_local(fut);
        },
    );
    tokio::task::spawn_local(async move {
        if let Err(error) = handle_io.await {
            eprintln!("[{}-bridge] ACP IO terminated: {}", kind, error);
        }
    });

    let init_req = acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_info(
        acp::Implementation::new("vibearound", env!("CARGO_PKG_VERSION")).title("VibeAround"),
    );
    let initialize = conn
        .initialize(init_req)
        .await
        .with_context(|| format!("ACP initialize failed for {}", kind))?;

    let startup_session_id = if let Some(resume_session_id) = resume_session_id.clone() {
        match conn
            .load_session(acp::LoadSessionRequest::new(
                resume_session_id.clone(),
                cwd.clone(),
            ))
            .await
        {
            Ok(_) => Some(resume_session_id),
            Err(error) => {
                eprintln!(
                    "[{}-bridge] failed to load session {}, bridge will start without session: {}",
                    kind, resume_session_id, error
                );
                None
            }
        }
    } else {
        None
    };

    let bridge = Arc::new(AcpBridge {
        conn,
        kind,
        initialize: initialize.clone(),
        session_id: Mutex::new(startup_session_id.clone()),
        provider_session_id_rx: Mutex::new(provider_session_id_rx),
        _worker_thread: Mutex::new(worker_thread),
        cancel_tx,
    });

    Ok(BridgeReady {
        bridge,
        startup_session_id,
        initialize,
    })
}
