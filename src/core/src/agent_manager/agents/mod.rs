//! Unified agent backend: all agents communicate via ACP (Agent Client Protocol).
//!
//! - Gemini: speaks ACP natively (`gemini --experimental-acp`)
//! - Claude: wrapped via `claude_acp` adapter (Claude SDK protocol → ACP translation)

pub mod claude_acp;
pub mod claude_sdk;
pub mod codex_acp;
pub mod codex_jsonl;
pub mod gemini_acp;
pub mod opencode_acp;
pub mod opencode_jsonl;
pub mod runtime_context;

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Gemini,
    OpenCode,
    Codex,
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKind::Claude => write!(f, "claude"),
            AgentKind::Gemini => write!(f, "gemini"),
            AgentKind::OpenCode => write!(f, "opencode"),
            AgentKind::Codex => write!(f, "codex"),
        }
    }
}

impl AgentKind {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "claude" | "claude-code" => Some(Self::Claude),
            "gemini" | "gemini-cli" => Some(Self::Gemini),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "codex" | "openai-codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn all() -> &'static [AgentKind] {
        &[AgentKind::Claude, AgentKind::Gemini, AgentKind::OpenCode, AgentKind::Codex]
    }

    pub fn enabled() -> Vec<AgentKind> {
        crate::config::ensure_loaded().enabled_agents.clone()
    }

    pub fn is_enabled(&self) -> bool {
        crate::config::ensure_loaded().enabled_agents.contains(self)
    }

    pub fn description(&self) -> &'static str {
        match self {
            AgentKind::Claude => "Anthropic Claude Code",
            AgentKind::Gemini => "Google Gemini CLI",
            AgentKind::OpenCode => "OpenCode AI Agent",
            AgentKind::Codex => "OpenAI Codex CLI",
        }
    }
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Text(String),
    Thinking(String),
    Progress(String),
    SessionReady { session_id: String },
    ToolUse { name: String, id: String, input: Option<String> },
    ToolResult { id: String, output: Option<String>, is_error: bool },
    TurnComplete {
        session_id: Option<String>,
        cost_usd: Option<f64>,
    },
    Error(String),
}

#[async_trait::async_trait]
pub trait AgentBackend: Send + Sync {
    async fn start(&mut self, cwd: &Path, system_prompt: Option<&str>) -> Result<Option<String>, String>;
    async fn send_message(&self, text: &str) -> Result<(), String>;
    async fn send_message_fire(&self, text: &str) -> Result<(), String>;
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent>;
    async fn shutdown(&mut self);
    fn kind(&self) -> AgentKind;
}

pub fn create_backend(kind: AgentKind) -> Box<dyn AgentBackend> {
    Box::new(AcpBackend::new(kind))
}

enum AcpCmd {
    Prompt {
        text: String,
        done_tx: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

pub struct AcpBackend {
    agent_kind: AgentKind,
    event_tx: broadcast::Sender<AgentEvent>,
    cmd_tx: Option<mpsc::Sender<AcpCmd>>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl AcpBackend {
    pub fn new(agent_kind: AgentKind) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            agent_kind,
            event_tx,
            cmd_tx: None,
            thread_handle: None,
        }
    }
}

#[async_trait::async_trait]
impl AgentBackend for AcpBackend {
    async fn start(&mut self, cwd: &Path, system_prompt: Option<&str>) -> Result<Option<String>, String> {
        let cwd = cwd.to_path_buf();
        let event_tx = self.event_tx.clone();
        let agent_kind = self.agent_kind;
        let system_prompt_owned = system_prompt.map(|s| s.to_string());
        let (cmd_tx, cmd_rx) = mpsc::channel::<AcpCmd>(32);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<Option<String>, String>>();

        let handle = std::thread::Builder::new()
            .name(format!("{}-acp", agent_kind))
            .spawn(move || {
                run_acp_thread(agent_kind, cwd, event_tx, cmd_rx, ready_tx, system_prompt_owned);
            })
            .map_err(|e| format!("Failed to spawn ACP thread: {}", e))?;

        self.cmd_tx = Some(cmd_tx);
        self.thread_handle = Some(handle);

        ready_rx
            .await
            .map_err(|_| "ACP thread died during init".to_string())?
    }

    async fn send_message(&self, text: &str) -> Result<(), String> {
        let cmd_tx = self.cmd_tx.as_ref().ok_or("Agent not started")?;
        let (done_tx, done_rx) = oneshot::channel();
        cmd_tx
            .send(AcpCmd::Prompt {
                text: text.to_string(),
                done_tx,
            })
            .await
            .map_err(|_| "ACP thread gone".to_string())?;
        done_rx.await.map_err(|_| "ACP thread gone".to_string())?
    }

    async fn send_message_fire(&self, text: &str) -> Result<(), String> {
        let cmd_tx = self.cmd_tx.as_ref().ok_or("Agent not started")?;
        let (done_tx, _done_rx) = oneshot::channel();
        cmd_tx
            .send(AcpCmd::Prompt {
                text: text.to_string(),
                done_tx,
            })
            .await
            .map_err(|_| "ACP thread gone".to_string())?;
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    async fn shutdown(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(AcpCmd::Shutdown).await;
        }
        if let Some(h) = self.thread_handle.take() {
            let _ = h.join();
        }
        eprintln!("[{}-acp] shutdown", self.agent_kind);
    }

    fn kind(&self) -> AgentKind {
        self.agent_kind
    }
}

fn run_acp_thread(
    agent_kind: AgentKind,
    cwd: PathBuf,
    event_tx: broadcast::Sender<AgentEvent>,
    cmd_rx: mpsc::Receiver<AcpCmd>,
    ready_tx: oneshot::Sender<Result<Option<String>, String>>,
    system_prompt: Option<String>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("Failed to build runtime: {}", e)));
            return;
        }
    };

    rt.block_on(async move {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                match acp_session_loop(agent_kind, cwd, event_tx, cmd_rx, ready_tx, system_prompt).await {
                    Ok(()) => {}
                    Err(e) => eprintln!("[{}-acp] session loop error: {}", agent_kind, e),
                }
            })
            .await;
    });
}

async fn acp_session_loop(
    agent_kind: AgentKind,
    cwd: PathBuf,
    event_tx: broadcast::Sender<AgentEvent>,
    mut cmd_rx: mpsc::Receiver<AcpCmd>,
    ready_tx: oneshot::Sender<Result<Option<String>, String>>,
    system_prompt: Option<String>,
) -> Result<(), String> {
    use agent_client_protocol as acp;
    use acp::Agent as _;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    if let Some(ref prompt) = system_prompt {
        match agent_kind {
            AgentKind::Gemini => {
                let prompt_path = cwd.join(".gemini").join("system.md");
                if let Some(parent) = prompt_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&prompt_path, prompt);
            }
            AgentKind::OpenCode => {
                let _ = std::fs::write(cwd.join("AGENTS.md"), prompt);
            }
            AgentKind::Codex => {
                let dir = cwd.join(".codex");
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join("instructions.md"), prompt);
            }
            AgentKind::Claude => {}
        }
    }

    let (read_stream, write_stream, _claude_thread, mut claude_real_session_id_rx): (
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
        Option<std::thread::JoinHandle<()>>,
        Option<mpsc::UnboundedReceiver<String>>,
    ) = match agent_kind {
        AgentKind::Claude => {
            let (r, w, h, sid_rx) = claude_acp::spawn_claude_acp(cwd.clone(), system_prompt);
            (r, w, Some(h), Some(sid_rx))
        }
        AgentKind::Gemini => {
            let system_md = system_prompt.as_ref().map(|_| cwd.join(".gemini").join("system.md"));
            let (r, w) = gemini_acp::spawn_gemini_process(&cwd, system_md.as_deref())?;
            (r, w, None, None)
        }
        AgentKind::OpenCode => {
            let (r, w) = opencode_acp::spawn_opencode_process(&cwd)?;
            (r, w, None, None)
        }
        AgentKind::Codex => {
            let (r, w) = codex_acp::spawn_codex_process(&cwd)?;
            (r, w, None, None)
        }
    };

    let client_handler = SharedAcpClientHandler {
        event_tx: event_tx.clone(),
    };
    let (conn, handle_io) = acp::ClientSideConnection::new(
        client_handler,
        write_stream.compat_write(),
        read_stream.compat(),
        |fut| {
            tokio::task::spawn_local(fut);
        },
    );
    tokio::task::spawn_local(handle_io);

    let _init_resp = conn
        .initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                .client_info(acp::Implementation::new("vibearound", "0.1.0").title("VibeAround")),
        )
        .await
        .map_err(|e| format!("ACP initialize failed: {}", e))?;

    let session_resp = conn
        .new_session(acp::NewSessionRequest::new(cwd))
        .await
        .map_err(|e| format!("ACP new_session failed: {}", e))?;

    let session_id = session_resp.session_id;
    let startup_session_id = if matches!(agent_kind, AgentKind::Claude) {
        None
    } else {
        Some(session_id.to_string())
    };
    let _ = ready_tx.send(Ok(startup_session_id));

    if let Some(mut session_id_rx) = claude_real_session_id_rx.take() {
        let event_tx = event_tx.clone();
        tokio::task::spawn_local(async move {
            let mut published_session_id: Option<String> = None;
            while let Some(discovered_session_id) = session_id_rx.recv().await {
                if published_session_id.as_deref() == Some(discovered_session_id.as_str()) {
                    continue;
                }
                published_session_id = Some(discovered_session_id.clone());
                let _ = event_tx.send(AgentEvent::SessionReady {
                    session_id: discovered_session_id,
                });
            }
        });
    }

    loop {
        let cmd = match cmd_rx.recv().await {
            Some(c) => c,
            None => break,
        };
        match cmd {
            AcpCmd::Prompt { text, done_tx } => {
                let text_content = acp::ContentBlock::Text(acp::TextContent::new(text));
                let result = conn
                    .prompt(acp::PromptRequest::new(
                        session_id.clone(),
                        vec![text_content],
                    ))
                    .await;
                match result {
                    Ok(_) => {
                        let _ = event_tx.send(AgentEvent::TurnComplete {
                            session_id: None,
                            cost_usd: None,
                        });
                        let _ = done_tx.send(Ok(()));
                    }
                    Err(e) => {
                        let err = format!("ACP prompt error: {}", e);
                        let _ = event_tx.send(AgentEvent::Error(err.clone()));
                        let _ = done_tx.send(Err(err));
                    }
                }
            }
            AcpCmd::Shutdown => break,
        }
    }

    Ok(())
}

struct SharedAcpClientHandler {
    event_tx: broadcast::Sender<AgentEvent>,
}

#[async_trait::async_trait(?Send)]
impl agent_client_protocol::Client for SharedAcpClientHandler {
    async fn request_permission(
        &self,
        args: agent_client_protocol::RequestPermissionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::RequestPermissionResponse> {
        let option_id = args
            .options
            .first()
            .map(|o| o.option_id.clone())
            .unwrap_or_else(|| "allow".into());
        Ok(agent_client_protocol::RequestPermissionResponse::new(
            agent_client_protocol::RequestPermissionOutcome::Selected(
                agent_client_protocol::SelectedPermissionOutcome::new(option_id),
            ),
        ))
    }

    async fn session_notification(
        &self,
        args: agent_client_protocol::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        use agent_client_protocol::{ContentBlock, SessionUpdate};

        match args.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(t) = chunk.content {
                    let _ = self.event_tx.send(AgentEvent::Text(t.text));
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if let ContentBlock::Text(t) = chunk.content {
                    let _ = self.event_tx.send(AgentEvent::Thinking(t.text));
                }
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let name = update.fields.title.clone().unwrap_or_else(|| "unknown".into());
                let id = update.tool_call_id.to_string();
                let has_output = update.fields.raw_output.is_some();
                let status_completed = update.fields.status.as_ref().map(|s| {
                    matches!(s, agent_client_protocol::ToolCallStatus::Completed | agent_client_protocol::ToolCallStatus::Failed)
                }).unwrap_or(false);

                if has_output || status_completed {
                    let output = update.fields.raw_output.as_ref().map(|v| {
                        if let Some(s) = v.as_str() { s.to_string() } else { v.to_string() }
                    }).or_else(|| {
                        update.fields.content.as_ref().map(|blocks| {
                            blocks.iter().filter_map(|block| {
                                match block {
                                    agent_client_protocol::ToolCallContent::Content(c) => {
                                        if let ContentBlock::Text(t) = &c.content { Some(t.text.clone()) } else { None }
                                    }
                                    _ => None,
                                }
                            }).collect::<Vec<_>>().join("")
                        })
                    });
                    let is_error = matches!(update.fields.status.as_ref(), Some(agent_client_protocol::ToolCallStatus::Failed));
                    let _ = self.event_tx.send(AgentEvent::ToolResult { id, output, is_error });
                } else {
                    let input = update.fields.raw_input.as_ref().map(|v| {
                        if let Some(s) = v.as_str() { s.to_string() } else { v.to_string() }
                    });
                    let _ = self.event_tx.send(AgentEvent::ToolUse { name, id, input });
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn write_text_file(
        &self,
        _: agent_client_protocol::WriteTextFileRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::WriteTextFileResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn read_text_file(
        &self,
        _: agent_client_protocol::ReadTextFileRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::ReadTextFileResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn create_terminal(
        &self,
        _: agent_client_protocol::CreateTerminalRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::CreateTerminalResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn terminal_output(
        &self,
        _: agent_client_protocol::TerminalOutputRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::TerminalOutputResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn release_terminal(
        &self,
        _: agent_client_protocol::ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::ReleaseTerminalResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn wait_for_terminal_exit(
        &self,
        _: agent_client_protocol::WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::WaitForTerminalExitResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn kill_terminal_command(
        &self,
        _: agent_client_protocol::KillTerminalCommandRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::KillTerminalCommandResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn ext_method(
        &self,
        _: agent_client_protocol::ExtRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::ExtResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn ext_notification(
        &self,
        _: agent_client_protocol::ExtNotification,
    ) -> agent_client_protocol::Result<()> {
        Ok(())
    }
}

pub struct JsonlBackend {
    agent_kind: AgentKind,
    event_tx: broadcast::Sender<AgentEvent>,
    cwd: Option<PathBuf>,
}

impl JsonlBackend {
    pub fn new(agent_kind: AgentKind) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self { agent_kind, event_tx, cwd: None }
    }
}

#[async_trait::async_trait]
impl AgentBackend for JsonlBackend {
    async fn start(&mut self, cwd: &Path, _system_prompt: Option<&str>) -> Result<Option<String>, String> {
        let cmd = match self.agent_kind {
            AgentKind::OpenCode => "opencode",
            AgentKind::Codex => "codex",
            _ => unreachable!(),
        };
        let check = tokio::process::Command::new("which")
            .arg(cmd)
            .output()
            .await
            .map_err(|e| format!("Failed to check {}: {}", cmd, e))?;
        if !check.status.success() {
            return Err(format!("{} not found in PATH", cmd));
        }
        self.cwd = Some(cwd.to_path_buf());
        Ok(None)
    }

    async fn send_message(&self, text: &str) -> Result<(), String> {
        let cwd = self.cwd.as_ref().ok_or("Agent not started")?;
        let event_tx = self.event_tx.clone();
        let agent_kind = self.agent_kind;

        let (cmd, args): (&str, Vec<String>) = match agent_kind {
            AgentKind::OpenCode => ("opencode", vec![
                "run".into(), "--format".into(), "json".into(), "--".into(), text.to_string(),
            ]),
            AgentKind::Codex => ("codex", vec![
                "exec".into(), "--json".into(), "--full-auto".into(), text.to_string(),
            ]),
            _ => unreachable!(),
        };

        let mut child = tokio::process::Command::new(cmd)
            .args(&args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {}", cmd, e))?;

        let stdout = child.stdout.take().ok_or("No stdout")?;
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() { continue; }
            let msg: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => {
                    if !line.trim().is_empty() {
                        let _ = event_tx.send(AgentEvent::Text(line));
                    }
                    continue;
                }
            };
            match agent_kind {
                AgentKind::OpenCode => opencode_jsonl::parse_event(&msg, &event_tx),
                AgentKind::Codex => codex_jsonl::parse_event(&msg, &event_tx),
                _ => {}
            }
        }

        let _status = child.wait().await.map_err(|e| format!("{} wait: {}", cmd, e))?;
        let _ = event_tx.send(AgentEvent::TurnComplete { session_id: None, cost_usd: None });
        Ok(())
    }

    async fn send_message_fire(&self, text: &str) -> Result<(), String> {
        let cwd = self.cwd.as_ref().ok_or("Agent not started")?.clone();
        let event_tx = self.event_tx.clone();
        let agent_kind = self.agent_kind;
        let text = text.to_string();

        tokio::spawn(async move {
            let (cmd, args): (&str, Vec<String>) = match agent_kind {
                AgentKind::OpenCode => ("opencode", vec![
                    "run".into(), "--format".into(), "json".into(), "--".into(), text,
                ]),
                AgentKind::Codex => ("codex", vec![
                    "exec".into(), "--json".into(), "--full-auto".into(), text,
                ]),
                _ => unreachable!(),
            };

            let mut child = match tokio::process::Command::new(cmd)
                .args(&args)
                .current_dir(&cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::Error(format!("Failed to spawn {}: {}", cmd, e)));
                    let _ = event_tx.send(AgentEvent::TurnComplete { session_id: None, cost_usd: None });
                    return;
                }
            };

            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() { continue; }
                    match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(msg) => match agent_kind {
                            AgentKind::OpenCode => opencode_jsonl::parse_event(&msg, &event_tx),
                            AgentKind::Codex => codex_jsonl::parse_event(&msg, &event_tx),
                            _ => {}
                        },
                        Err(_) => {
                            if !line.trim().is_empty() {
                                let _ = event_tx.send(AgentEvent::Text(line));
                            }
                        }
                    }
                }
            }

            let _ = child.wait().await;
            let _ = event_tx.send(AgentEvent::TurnComplete { session_id: None, cost_usd: None });
        });

        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    async fn shutdown(&mut self) {
        self.cwd = None;
    }

    fn kind(&self) -> AgentKind {
        self.agent_kind
    }
}
