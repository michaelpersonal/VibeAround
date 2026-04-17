//! Agent integration management — MCP config, skill files, and ACP agent npm packages.
//!
//! Syncs VibeAround integrations into each coding agent's global settings.
//! Identifies managed entries by the "vibearound" key name in MCP server
//! configs and the "vibearound" skill directory name.
//!
//! ## Module layout
//!
//! - [`mcp`]    — install/uninstall the VibeAround MCP server entry into
//!                each agent's global config (JSON or TOML).
//! - [`skills`] — install/uninstall the `SKILL.md` files each agent consumes.

mod mcp;
mod skills;

use anyhow::Context;

use crate::{config, resources};

use mcp::{install_mcp_config, uninstall_mcp_config};
use skills::{install_skill, uninstall_skill};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Sync all agent integrations with the current settings.
/// - Enabled agents: install/update MCP config + skills.
/// - Disabled agents: remove MCP config + skills.
pub fn sync_integrations(settings: &serde_json::Value) {
    let port = config::DEFAULT_PORT;
    // The /mcp endpoint is bearer-gated by the web server auth middleware
    // (see server/src/web_server/auth.rs). Coding agents (Claude Code,
    // Gemini, Codex, Cursor, Kiro, Qwen) drive MCP over plain HTTP and
    // rarely support attaching Authorization headers uniformly from a
    // config file — particularly Codex which reads TOML. The middleware
    // already accepts the same token via `?token=<hex>` (same path that
    // the SPA and WebSocket clients use), so we bake it into the URL we
    // write into each agent's config. The token rotates on every daemon
    // start, so `sync_integrations` runs on every startup and rewrites
    // all configs with the fresh value. `auth.json` is 0600 on disk and
    // the config files inherit the same mode when we control writes, so
    // leaking the token via `ps` / loopback-only traffic is acceptable.
    let mcp_url = match crate::auth::read_token_file() {
        Some(auth) => format!("http://127.0.0.1:{}/va/mcp?token={}", port, auth.token),
        None => {
            eprintln!(
                "[integrations] auth.json missing — writing MCP config without token; \
                 coding agents will get 401 until the daemon rewrites it"
            );
            format!("http://127.0.0.1:{}/va/mcp", port)
        }
    };

    let all_agents = resources::agent_ids();
    let enabled_agents = resolve_enabled_agents(settings, &all_agents);

    for agent in &all_agents {
        let enabled = enabled_agents.iter().any(|a| a == agent);
        if enabled {
            if let Err(e) = install_mcp_config(agent, &mcp_url) {
                eprintln!("[integrations] MCP config install for {}: {:#}", agent, e);
            }
            if let Err(e) = install_skill(agent) {
                eprintln!("[integrations] skill install for {}: {:#}", agent, e);
            }
        } else {
            if let Err(e) = uninstall_mcp_config(agent) {
                eprintln!(
                    "[integrations] MCP config uninstall for {}: {:#}",
                    agent, e
                );
            }
            if let Err(e) = uninstall_skill(agent) {
                eprintln!("[integrations] skill uninstall for {}: {:#}", agent, e);
            }
        }
    }
}

/// Output captured from an install command.
pub struct InstallOutput {
    pub stdout: String,
    pub stderr: String,
}

/// Auto-install an npm ACP agent package into `~/.vibearound/plugins/`.
pub async fn auto_install_npm_agent(npm_package: &str) -> anyhow::Result<()> {
    auto_install_npm_agent_with_output(npm_package)
        .await
        .map(|_| ())
}

/// Like `auto_install_npm_agent` but returns captured stdout/stderr.
pub async fn auto_install_npm_agent_with_output(
    npm_package: &str,
) -> anyhow::Result<InstallOutput> {
    let plugins_dir = crate::env::acp_agents_dir();
    std::fs::create_dir_all(&plugins_dir).with_context(|| format!("creating {:?}", plugins_dir))?;

    let pkg_json = plugins_dir.join("package.json");
    if !pkg_json.exists() {
        let init = serde_json::json!({ "name": "vibearound-plugins", "private": true });
        std::fs::write(&pkg_json, serde_json::to_string_pretty(&init).unwrap())
            .context("writing package.json")?;
    }

    let output = crate::env::command("npm")
        .args(["install", npm_package])
        .current_dir(&plugins_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .with_context(|| format!("running npm install {}", npm_package))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        anyhow::bail!("npm install {} failed: {}", npm_package, stderr.trim());
    }
    eprintln!("[integrations] installed {}", npm_package);
    Ok(InstallOutput { stdout, stderr })
}

/// Install a native agent CLI by running its official install command.
pub async fn auto_install_agent_cmd(install_cmd: &str, agent: &str) -> anyhow::Result<()> {
    auto_install_agent_cmd_with_output(install_cmd, agent)
        .await
        .map(|_| ())
}

/// Like `auto_install_agent_cmd` but returns captured stdout/stderr.
pub async fn auto_install_agent_cmd_with_output(
    install_cmd: &str,
    agent: &str,
) -> anyhow::Result<InstallOutput> {
    eprintln!(
        "[integrations] running install for {}: {}",
        agent, install_cmd
    );

    let output = crate::env::command("sh")
        .args(["-c", install_cmd])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .with_context(|| format!("running install cmd for {}", agent))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        anyhow::bail!("install {} failed: {}", agent, stderr.trim());
    }

    eprintln!("[integrations] installed {}", agent);
    Ok(InstallOutput { stdout, stderr })
}

/// Check if a program is available in PATH.
pub fn is_program_available(program: &str) -> bool {
    crate::env::std_command("which")
        .arg(program)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Pre-install all ACP agent packages (npm or binary) for enabled agents.
pub async fn install_acp_agents(settings: &serde_json::Value) {
    let all_agents = resources::agent_ids();
    let enabled_agents = resolve_enabled_agents(settings, &all_agents);

    for agent_id in &enabled_agents {
        let agent_def = match resources::agent_by_id(agent_id) {
            Some(def) => def,
            None => continue,
        };

        // npm-based agents (Claude ACP, Codex ACP)
        if let Some(npm_pkg) = &agent_def.acp.npm_package {
            let bin_name = agent_def.acp.bin_name.as_deref().unwrap_or(npm_pkg);
            if crate::env::resolve_acp_agent_bin(bin_name).is_ok() {
                continue;
            }
            eprintln!("[integrations] installing ACP agent: {}", npm_pkg);
            if let Err(e) = auto_install_npm_agent(npm_pkg).await {
                eprintln!("[integrations] npm install {} error: {}", npm_pkg, e);
            }
        }
        // Native binary agents with install command (Cursor, Kiro)
        else if let Some(install_cmd) = &agent_def.acp.install_cmd {
            if is_program_available(&agent_def.acp.program) {
                continue;
            }
            if let Err(e) = auto_install_agent_cmd(install_cmd, agent_id).await {
                eprintln!("[integrations] install {} error: {}", agent_id, e);
            }
        }
    }
}

/// Resolve which agents are enabled from settings JSON.
/// Falls back to all agents if `enabled_agents` is not set.
pub fn resolve_enabled_agents(settings: &serde_json::Value, all_agents: &[&str]) -> Vec<String> {
    settings
        .get("enabled_agents")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| all_agents.iter().map(|s| s.to_string()).collect())
}
