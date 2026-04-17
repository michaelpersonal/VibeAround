//! `StdioPluginRuntime` — the lifetime handle for a spawned stdio plugin.
//!
//! `spawn()` starts the Node child process, registers it with the global
//! `ChildRegistry` (for authoritative SIGKILL on daemon stop / Tauri exit),
//! and fires the ACP bridge thread.
//!
//! `shutdown()` aborts the guardian task, which drops the `Child` through
//! the registry → `kill_on_drop(true)` fires → child dies.

use std::sync::Arc;

use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::acp_hub::ACPHub;
use crate::child_registry::{ChildKind, ChildRegistry};

use super::super::manifest::ChannelPluginManifest;
use super::super::plugin_host::PluginHost;
use super::super::{ChannelInput, ChannelOutput};
use super::bridge::run_acp_plugin_bridge;

/// A running stdio plugin connected via ACP protocol.
#[derive(Debug)]
pub struct StdioPluginRuntime {
    channel_kind: String,
    /// Send `ChannelOutput` to the plugin via ACP session_notification.
    output_tx: mpsc::UnboundedSender<ChannelOutput>,
    abort_handle: AbortHandle,
}

impl StdioPluginRuntime {
    pub async fn spawn(
        manifest: ChannelPluginManifest,
        input_tx: mpsc::UnboundedSender<ChannelInput>,
        acp_hub: Arc<ACPHub>,
        plugin_host: Arc<PluginHost>,
    ) -> Result<Self, String> {
        if manifest.runtime != "node" {
            return Err(format!(
                "unsupported channel runtime '{}' for {}",
                manifest.runtime, manifest.channel_kind
            ));
        }

        if !manifest.entry_path.exists() {
            return Err(format!(
                "plugin entry not found: {}",
                manifest.entry_path.display()
            ));
        }

        let mut child = crate::env::command("node")
            .arg(&manifest.entry_path)
            .current_dir(&manifest.plugin_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("failed to spawn plugin process: {}", error))?;

        let stdin = child.stdin.take().ok_or("plugin stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("plugin stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("plugin stderr unavailable")?;

        let channel_kind = manifest.channel_kind.clone();

        // Register in the global ChildRegistry. This is the canonical owner
        // of the `Child` handle now — the registry guarantees the process is
        // SIGKILLed on daemon stop + Tauri RunEvent::Exit even if the tokio
        // runtime tears down without polling this task's destructor.
        let registry_id =
            ChildRegistry::global().register(ChildKind::Plugin, channel_kind.clone(), child);

        // Stderr → log
        let stderr_channel = channel_kind.clone();
        tokio::spawn(async move {
            let reader = tokio::io::BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[{}][plugin] {}", stderr_channel, line);
            }
        });

        // Channel for outbound ChannelOutput → ACP session_notification.
        let (output_tx, output_rx) = mpsc::unbounded_channel::<ChannelOutput>();

        // Guardian task — on abort (via `shutdown()` → `abort_handle.abort()`)
        // it removes the child from the registry and drops it, triggering
        // `kill_on_drop`. This is the happy-path shutdown.
        //
        // If the runtime tears down abruptly instead, the ChildRegistry
        // kill_all() path (fired from `RunningDaemon::stop` and Tauri Exit)
        // still kills the process synchronously. Either way, no orphans.
        let guardian_channel = channel_kind.clone();
        let guardian_task = tokio::spawn(async move {
            struct Guard(u64, String);
            impl Drop for Guard {
                fn drop(&mut self) {
                    if let Some(_child) = ChildRegistry::global().remove(self.0) {
                        eprintln!(
                            "[{}] guardian drop: plugin child killed via registry",
                            self.1
                        );
                    }
                }
            }
            let _guard = Guard(registry_id, guardian_channel);
            std::future::pending::<()>().await;
        });
        let abort_handle = guardian_task.abort_handle();

        // Spawn the ACP bridge on a dedicated thread (needs LocalSet for !Send futures).
        let acp_channel = channel_kind.clone();
        let raw_config = manifest.raw_config.clone();
        std::thread::Builder::new()
            .name(format!("{}-plugin", channel_kind))
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to build plugin runtime");
                runtime.block_on(async move {
                    run_acp_plugin_bridge(
                        acp_channel,
                        raw_config,
                        stdin,
                        stdout,
                        input_tx,
                        output_rx,
                        acp_hub,
                        plugin_host,
                    )
                    .await;
                });
            })
            .map_err(|e| format!("Failed to spawn plugin thread: {}", e))?;

        Ok(Self {
            channel_kind,
            output_tx,
            abort_handle,
        })
    }

    pub fn abort_handle(&self) -> AbortHandle {
        self.abort_handle.clone()
    }

    pub async fn send_output(&self, output: ChannelOutput) {
        if let Err(error) = self.output_tx.send(output) {
            eprintln!(
                "[{}] failed to send output to ACP plugin bridge: {}",
                self.channel_kind, error
            );
        }
    }

    pub async fn shutdown(&self) {
        self.abort_handle.abort();
    }
}
