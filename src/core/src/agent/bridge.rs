//! `AcpAgentBridge` — the manager-owned `ProcessBridge` for ACP coding
//! CLIs.
//!
//! On spawn the supervisor hands this bridge a `StdioPipes`. The bridge
//! runs a dedicated std::thread with `current_thread` tokio runtime + a
//! `LocalSet` (needed for ACP's `!Send` internal futures), wires the
//! pipes directly into an `acp::ClientSideConnection`, performs the
//! initial handshake, optionally resumes a session, then publishes the
//! assembled [`AgentReady`] to the caller via a oneshot before entering
//! the IO loop.
//!
//! Unlike [`ChannelPluginBridge`], the agent has **no restart policy**
//! ([`RestartPolicy::Never`]). One bridge = one spawn = one `AgentReady`.
//! The `BridgeFactory` handed to [`Supervisor::register`] is therefore
//! single-shot: we build the bridge eagerly, stash it in a `Mutex<Option>`,
//! and `take()` it the first (and only) time the factory is invoked.
//!
//! [`ChannelPluginBridge`]: crate::channels::plugin_bridge::ChannelPluginBridge
//! [`RestartPolicy::Never`]: crate::process::RestartPolicy::Never
//! [`Supervisor::register`]: crate::process::Supervisor::register

use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol as acp;
use anyhow::{anyhow, Context};
use tokio::sync::oneshot;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::process::bridge::{BridgeExit, BridgeFuture, CancelSignal, ProcessBridge, StdioPipes};

use super::runtime::{Agent, AgentClient, AgentClientHandler, AgentReady};

/// Single-shot bridge that initializes an ACP connection on its own
/// thread, then runs the IO loop until cancelled or the child exits.
pub struct AcpAgentBridge {
    pub agent_id: String,
    pub cwd: PathBuf,
    pub resume_session_id: Option<String>,
    pub client_handler: Arc<dyn AgentClientHandler>,
    pub ready_tx: oneshot::Sender<anyhow::Result<AgentReady>>,
}

impl ProcessBridge for AcpAgentBridge {
    fn run(self: Box<Self>, pipes: StdioPipes, cancel: CancelSignal) -> BridgeFuture {
        let this = *self;
        Box::pin(async move {
            let (exit_tx, exit_rx) = oneshot::channel::<BridgeExit>();
            let thread_name = format!("{}-agent-bridge", this.agent_id);

            // If we fail to even build the runtime / spawn the thread,
            // translate that into a ready-channel error so the caller
            // bubbles up a real message instead of a "channel closed".
            let AcpAgentBridge {
                agent_id,
                cwd,
                resume_session_id,
                client_handler,
                ready_tx,
            } = this;

            let spawn_result = std::thread::Builder::new().name(thread_name).spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(anyhow!(
                            "failed to build agent bridge runtime: {}",
                            e
                        )));
                        let _ = exit_tx.send(BridgeExit::ProtocolError(anyhow!(
                            "agent bridge runtime build failed: {}",
                            e
                        )));
                        return;
                    }
                };
                rt.block_on(async move {
                    let local = tokio::task::LocalSet::new();
                    local
                        .run_until(async move {
                            let exit = drive_agent_bridge(
                                agent_id,
                                cwd,
                                resume_session_id,
                                client_handler,
                                ready_tx,
                                pipes,
                                cancel,
                            )
                            .await;
                            let _ = exit_tx.send(exit);
                        })
                        .await;
                });
            });

            if let Err(e) = spawn_result {
                return BridgeExit::ProtocolError(anyhow!(
                    "failed to spawn agent bridge thread: {}",
                    e
                ));
            }

            exit_rx.await.unwrap_or(BridgeExit::Clean)
        })
    }
}

async fn drive_agent_bridge(
    agent_id: String,
    cwd: PathBuf,
    resume_session_id: Option<String>,
    client_handler: Arc<dyn AgentClientHandler>,
    ready_tx: oneshot::Sender<anyhow::Result<AgentReady>>,
    pipes: StdioPipes,
    mut cancel: CancelSignal,
) -> BridgeExit {
    use acp::Agent as _;

    // Hand stdio directly to ACP — channel plugins do the same and it
    // avoids a duplex-copy indirection on every read/write.
    let (conn, handle_io) = acp::ClientSideConnection::new(
        AgentClient::new(client_handler),
        pipes.stdin.compat_write(),
        pipes.stdout.compat(),
        |fut| {
            tokio::task::spawn_local(fut);
        },
    );

    // Drive ACP IO on its own local task. When the child exits, the pipes
    // close and `handle_io` completes — we observe that via the join
    // below to emit the correct BridgeExit.
    let io_label = agent_id.clone();
    let io_task = tokio::task::spawn_local(async move {
        let result = handle_io.await;
        if let Err(ref error) = result {
            tracing::info!("[{}-agent] ACP IO terminated: {}", io_label, error);
        }
        result
    });

    // ACP handshake.
    let init_req = acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_info(
        acp::Implementation::new("vibearound", env!("CARGO_PKG_VERSION")).title("VibeAround"),
    );
    let initialize = match conn
        .initialize(init_req)
        .await
        .with_context(|| format!("ACP initialize failed for {}", agent_id))
    {
        Ok(r) => r,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return BridgeExit::ProtocolError(anyhow!("ACP initialize failed"));
        }
    };

    // Optional session resume. A failed resume downgrades to "start
    // fresh" — matching the pre-migration behavior.
    let startup_session_id = if let Some(resume) = resume_session_id.clone() {
        match conn
            .load_session(acp::LoadSessionRequest::new(resume.clone(), cwd.clone()))
            .await
        {
            Ok(_) => Some(resume),
            Err(error) => {
                tracing::info!(
                    "[{}-agent] failed to load session {}, starting without session: {}",
                    agent_id, resume, error
                );
                None
            }
        }
    } else {
        None
    };

    // Assemble the Agent and hand it back to the caller.
    let agent = Agent::from_connection(
        conn,
        agent_id.clone(),
        initialize.clone(),
        startup_session_id.clone(),
    );
    let ready = AgentReady {
        agent,
        startup_session_id,
        initialize,
    };
    if ready_tx.send(Ok(ready)).is_err() {
        // The caller went away between spawn and ready — cancel so
        // the supervisor records Stopped and the child is reaped.
        return BridgeExit::Cancelled;
    }

    // Main loop: hold the bridge alive until either the child dies
    // (io_task completes) or the supervisor asks us to stop.
    tokio::select! {
        io_result = io_task => match io_result {
            Ok(Ok(())) => BridgeExit::Clean,
            Ok(Err(error)) => BridgeExit::ProtocolError(anyhow!("ACP IO terminated: {}", error)),
            Err(join_error) => BridgeExit::ProtocolError(anyhow!("ACP IO task join error: {}", join_error)),
        },
        _ = cancel.wait_for(|v| *v) => {
            tracing::info!("[{}-agent] cancelled by supervisor", agent_id);
            BridgeExit::Cancelled
        }
    }
}
