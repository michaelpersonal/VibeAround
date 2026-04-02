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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Claude,
    Gemini,
    OpenCode,
    Codex,
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Gemini => write!(f, "gemini"),
            Self::OpenCode => write!(f, "opencode"),
            Self::Codex => write!(f, "codex"),
        }
    }
}

impl AgentKind {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        // Look up by alias in resources, then map the agent ID to the enum variant
        let agent = crate::resources::agent_by_alias(s)?;
        Self::from_id(&agent.id)
    }

    /// Map an agent ID string to the enum variant.
    fn from_id(id: &str) -> Option<Self> {
        match id {
            "claude" => Some(Self::Claude),
            "gemini" => Some(Self::Gemini),
            "opencode" => Some(Self::OpenCode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn all() -> &'static [AgentKind] {
        &[Self::Claude, Self::Gemini, Self::OpenCode, Self::Codex]
    }

    pub fn enabled() -> Vec<AgentKind> {
        crate::config::ensure_loaded().enabled_agents.clone()
    }

    pub fn is_enabled(&self) -> bool {
        crate::config::ensure_loaded().enabled_agents.contains(self)
    }

    pub fn display_name(&self) -> &str {
        crate::resources::agent_by_id(&self.to_string())
            .expect("AgentKind variant missing from agents.json")
            .display_name.as_str()
    }

    pub fn description(&self) -> &str {
        crate::resources::agent_by_id(&self.to_string())
            .expect("AgentKind variant missing from agents.json")
            .description.as_str()
    }
}

// ---------------------------------------------------------------------------
// AgentProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait AgentProvider: Send + Sync {
    fn kind(&self) -> AgentKind;

    async fn connect(
        &self,
        workspace: &Path,
    ) -> anyhow::Result<ProviderConnection>;
}

pub fn provider_for_kind(kind: AgentKind) -> Arc<dyn AgentProvider> {
    Arc::new(StdioAcpProvider::new(kind))
}

// ---------------------------------------------------------------------------
// StdioAcpProvider — generic provider for CLIs that speak ACP over stdio
// ---------------------------------------------------------------------------

struct StdioAcpProvider {
    agent_kind: AgentKind,
}

impl StdioAcpProvider {
    fn new(kind: AgentKind) -> Self {
        Self { agent_kind: kind }
    }
}

#[async_trait]
impl AgentProvider for StdioAcpProvider {
    fn kind(&self) -> AgentKind { self.agent_kind }

    async fn connect(
        &self,
        workspace: &Path,
    ) -> anyhow::Result<ProviderConnection> {
        let agent_def = crate::resources::agent_by_id(&self.agent_kind.to_string())
            .ok_or_else(|| anyhow!("No resource definition for agent '{}'", self.agent_kind))?;
        let args: Vec<&str> = agent_def.acp.args.iter().map(|s| s.as_str()).collect();
        let (read_stream, write_stream) =
            spawn_stdio_acp(self.agent_kind, &agent_def.acp.program, &args, workspace)?;
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
    kind: AgentKind,
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> anyhow::Result<(DuplexStream, DuplexStream)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    eprintln!("[{}-acp] spawning {} {} in {:?}", kind, program, args.join(" "), cwd);
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("Failed to spawn {} {}. Is it installed?", program, args.join(" ")))?;
    eprintln!("[{}-acp] process spawned pid={:?}", kind, child.id());

    let child_stdout = child.stdout.take().context("Process has no stdout")?;
    let child_stdin = child.stdin.take().context("Process has no stdin")?;

    // stdout → client_read
    let (client_read, mut bridge_write) = tokio::io::duplex(64 * 1024);
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
        drop(child);
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
