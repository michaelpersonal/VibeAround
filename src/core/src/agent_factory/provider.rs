use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use tokio::io::DuplexStream;
use tokio::sync::mpsc;

/// External CLI/provider session identifier.
pub type ProviderSessionId = String;

/// Low-level ACP transport connection returned by a provider wrapper.
pub struct ProviderConnection {
    pub read_stream: DuplexStream,
    pub write_stream: DuplexStream,
    pub session_id_rx: Option<mpsc::UnboundedReceiver<ProviderSessionId>>,
    pub worker_thread: Option<std::thread::JoinHandle<()>>,
}

// ---------------------------------------------------------------------------
// AgentProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait AgentProvider: Send + Sync {
    /// The agent's ID as defined in `resources/agents.json`.
    fn id(&self) -> &str;

    async fn connect(
        &self,
        workspace: &Path,
        extra_env: &[(&str, &str)],
    ) -> anyhow::Result<ProviderConnection>;
}

/// Build a provider for the given agent ID. The ID must match an entry in
/// `resources/agents.json` — validate with `resources::agent_by_id` at
/// boundaries (config load, API request) if the source is untrusted.
pub fn provider_for_id(id: impl Into<String>) -> Arc<dyn AgentProvider> {
    Arc::new(StdioAcpProvider::new(id.into()))
}

// ---------------------------------------------------------------------------
// StdioAcpProvider — generic provider for CLIs that speak ACP over stdio
// ---------------------------------------------------------------------------

struct StdioAcpProvider {
    agent_id: String,
}

impl StdioAcpProvider {
    fn new(agent_id: String) -> Self {
        Self { agent_id }
    }
}

#[async_trait]
impl AgentProvider for StdioAcpProvider {
    fn id(&self) -> &str {
        &self.agent_id
    }

    async fn connect(
        &self,
        workspace: &Path,
        extra_env: &[(&str, &str)],
    ) -> anyhow::Result<ProviderConnection> {
        let agent_def = crate::resources::agent_by_id(&self.agent_id)
            .ok_or_else(|| anyhow!("No resource definition for agent '{}'", self.agent_id))?;

        // Resolve program + args based on install method:
        // 1. npm-based agents → `node <resolved_entry>` (Claude ACP, Codex ACP)
        // 2. binary-download agents → binary from ~/.vibearound/bin/ (Cursor, Kiro)
        // 3. native agents → program + args from PATH (Gemini, OpenCode)
        let (program, resolved_args) = if let Some(npm_pkg) = &agent_def.acp.npm_package {
            let bin_name = agent_def.acp.bin_name.as_deref().unwrap_or(npm_pkg);
            if crate::env::resolve_acp_agent_bin(bin_name).is_err() {
                eprintln!("[{}-acp] auto-installing {} ...", self.agent_id, npm_pkg);
                crate::agent_integrations::auto_install_npm_agent(npm_pkg).await?;
            }
            let entry = crate::env::resolve_acp_agent_bin(bin_name)
                .with_context(|| format!("Resolving ACP agent '{}' (npm: {})", self.agent_id, npm_pkg))?;
            ("node".to_string(), vec![entry.to_string_lossy().to_string()])
        } else if let Some(install_cmd) = &agent_def.acp.install_cmd {
            if !crate::agent_integrations::is_program_available(&agent_def.acp.program) {
                eprintln!("[{}-acp] auto-installing via install cmd ...", self.agent_id);
                crate::agent_integrations::auto_install_agent_cmd(install_cmd, &self.agent_id).await?;
            }
            (agent_def.acp.program.clone(), agent_def.acp.args.clone())
        } else {
            (agent_def.acp.program.clone(), agent_def.acp.args.clone())
        };

        let args_refs: Vec<&str> = resolved_args.iter().map(|s| s.as_str()).collect();
        let (read_stream, write_stream) =
            spawn_stdio_acp(&self.agent_id, &program, &args_refs, workspace, extra_env)?;
        Ok(ProviderConnection {
            read_stream,
            write_stream,
            session_id_rx: None,
            worker_thread: None,
        })
    }
}

/// Spawn a CLI that speaks ACP over stdio, return duplex streams.
fn spawn_stdio_acp(
    agent_id: &str,
    program: &str,
    args: &[&str],
    cwd: &Path,
    extra_env: &[(&str, &str)],
) -> anyhow::Result<(DuplexStream, DuplexStream)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    eprintln!("[{}-acp] spawning {} {} in {:?}", agent_id, program, args.join(" "), cwd);
    let mut cmd = crate::env::command(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn()
        .with_context(|| format!("Failed to spawn {} {}. Is it installed?", program, args.join(" ")))?;
    eprintln!("[{}-acp] process spawned pid={:?}", agent_id, child.id());

    let child_stdout = child.stdout.take().context("Process has no stdout")?;
    let child_stdin = child.stdin.take().context("Process has no stdin")?;

    // Transfer ownership of `Child` to the global ChildRegistry. kill_on_drop
    // alone is not enough: the old code moved `child` into the stdout reader
    // closure, which only dropped it on stdout EOF. On abrupt runtime teardown
    // that task never ran its destructor, leaving PPID=1 orphans.
    // The registry's kill_all() path synchronously SIGKILLs every child on
    // daemon stop + Tauri Exit, regardless of task scheduler state.
    let registry_id = crate::child_registry::ChildRegistry::global().register(
        crate::child_registry::ChildKind::AgentAcp,
        format!("{}-acp", agent_id),
        child,
    );

    // stdout → client_read
    let (client_read, mut bridge_write) = tokio::io::duplex(64 * 1024);
    let agent_id_owned = agent_id.to_string();
    tokio::task::spawn_local(async move {
        let mut stdout = child_stdout;
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if bridge_write.write_all(&buf[..n]).await.is_err() { break; }
                }
                Err(_) => break,
            }
        }
        // Clean shutdown path: pull the child out of the registry and drop
        // it. kill_on_drop fires if the process is still alive.
        if let Some(_c) = crate::child_registry::ChildRegistry::global().remove(registry_id) {
            eprintln!("[{}-acp] stdout EOF — dropping child via registry", agent_id_owned);
        }
    });

    // client_write → stdin
    let (mut bridge_read, client_write) = tokio::io::duplex(64 * 1024);
    tokio::task::spawn_local(async move {
        let mut stdin = child_stdin;
        let mut buf = [0u8; 8192];
        loop {
            match bridge_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if stdin.write_all(&buf[..n]).await.is_err() { break; }
                    let _ = stdin.flush().await;
                }
                Err(_) => break,
            }
        }
    });

    Ok((client_read, client_write))
}
