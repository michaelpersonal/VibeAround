//! Agent integration management — MCP config, skill files, and ACP agent npm packages.
//!
//! Syncs VibeAround integrations (MCP server entry, skill files, npm packages)
//! into each coding agent's global settings. Lives in core so both the desktop
//! app and a standalone server can call it.

use std::path::PathBuf;

use anyhow::{anyhow, Context};

use crate::{config, resources};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Sync all agent integrations with the current settings.
/// Installs MCP config + skills for enabled agents, removes for disabled ones.
/// Errors are non-fatal and logged per-agent.
pub fn sync_integrations(settings: &serde_json::Value) {
    let port = config::DEFAULT_PORT;
    let mcp_url = format!("http://127.0.0.1:{}/mcp", port);

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
                eprintln!("[integrations] MCP config uninstall for {}: {:#}", agent, e);
            }
            if let Err(e) = uninstall_skill(agent) {
                eprintln!("[integrations] skill uninstall for {}: {:#}", agent, e);
            }
        }
    }
}

/// Auto-install an npm ACP agent package into `~/.vibearound/plugins/`.
/// Called lazily on first use when the binary isn't found.
pub async fn auto_install_npm_agent(npm_package: &str) -> anyhow::Result<()> {
    let plugins_dir = crate::env::acp_agents_dir();
    std::fs::create_dir_all(&plugins_dir)
        .with_context(|| format!("creating {:?}", plugins_dir))?;

    // Ensure package.json exists
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

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("npm install {} failed: {}", npm_package, stderr.trim());
    }
    eprintln!("[integrations] installed {}", npm_package);
    Ok(())
}

/// Pre-install all npm-based ACP agent packages for enabled agents.
pub async fn install_acp_agents(settings: &serde_json::Value) {
    let all_agents = resources::agent_ids();
    let enabled_agents = resolve_enabled_agents(settings, &all_agents);

    for agent_id in &enabled_agents {
        if let Some(agent_def) = resources::agent_by_id(agent_id) {
            if let Some(npm_pkg) = &agent_def.acp.npm_package {
                let bin_name = agent_def.acp.bin_name.as_deref().unwrap_or(npm_pkg);
                // Skip if already installed
                if crate::env::resolve_acp_agent_bin(bin_name).is_ok() {
                    continue;
                }
                eprintln!("[integrations] installing ACP agent: {}", npm_pkg);
                if let Err(e) = auto_install_npm_agent(npm_pkg).await {
                    eprintln!("[integrations] npm install {} error: {}", npm_pkg, e);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn resolve_enabled_agents(settings: &serde_json::Value, all_agents: &[&str]) -> Vec<String> {
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

fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| anyhow!("Cannot determine home directory"))
}

/// Merge VibeAround MCP server entry into an agent's global settings JSON file.
fn install_mcp_config(agent: &str, mcp_url: &str) -> anyhow::Result<()> {
    let home = home_dir()?;

    let agent_def = match resources::agent_by_id(agent) {
        Some(def) => def,
        None => return Ok(()),
    };
    let global_config = match &agent_def.global_config {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let config_path = home.join(&global_config.settings_path);
    let mcp_key = &global_config.mcp_key;

    let mcp_value_str = serde_json::to_string(&global_config.mcp_entry)
        .context("serialize mcp_entry")?;
    let mcp_value: serde_json::Value =
        serde_json::from_str(&mcp_value_str.replace("{mcp_url}", mcp_url))
            .context("parse mcp_entry after substitution")?;

    let data = std::fs::read_to_string(&config_path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value =
        serde_json::from_str(&data).unwrap_or(serde_json::json!({}));

    if let Some(obj) = root.as_object_mut() {
        let servers = obj
            .entry(mcp_key)
            .or_insert_with(|| serde_json::json!({}));
        if let Some(servers_obj) = servers.as_object_mut() {
            servers_obj.insert("vibearound".to_string(), mcp_value);
        }
    }

    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let pretty = serde_json::to_string_pretty(&root).context("JSON serialize")?;
    std::fs::write(&config_path, pretty)
        .with_context(|| format!("Write {:?}", config_path))?;

    eprintln!(
        "[integrations] Installed MCP config for {} at {:?}",
        agent, config_path
    );
    Ok(())
}

/// Remove VibeAround MCP server entry from an agent's global settings JSON file.
fn uninstall_mcp_config(agent: &str) -> anyhow::Result<()> {
    let home = home_dir()?;

    let agent_def = match resources::agent_by_id(agent) {
        Some(def) => def,
        None => return Ok(()),
    };
    let global_config = match &agent_def.global_config {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let config_path = home.join(&global_config.settings_path);
    let mcp_key = &global_config.mcp_key;

    if !config_path.exists() {
        return Ok(());
    }

    let data = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Read {:?}", config_path))?;
    let mut root: serde_json::Value =
        serde_json::from_str(&data).unwrap_or(serde_json::json!({}));

    let mut changed = false;
    if let Some(obj) = root.as_object_mut() {
        if let Some(servers) = obj.get_mut(mcp_key) {
            if let Some(servers_obj) = servers.as_object_mut() {
                if servers_obj.remove("vibearound").is_some() {
                    changed = true;
                }
            }
        }
    }

    if changed {
        let pretty = serde_json::to_string_pretty(&root).context("JSON serialize")?;
        std::fs::write(&config_path, pretty)
            .with_context(|| format!("Write {:?}", config_path))?;
        eprintln!(
            "[integrations] Removed MCP config for {} at {:?}",
            agent, config_path
        );
    }

    Ok(())
}

/// Install the vibearound skill file for a given agent.
fn install_skill(agent: &str) -> anyhow::Result<()> {
    let agent_def = match resources::agent_by_id(agent) {
        Some(def) => def,
        None => return Ok(()),
    };
    let skill_dir_rel = match &agent_def.global_config {
        Some(cfg) => match &cfg.skill_dir {
            Some(dir) => dir,
            None => return Ok(()),
        },
        None => return Ok(()),
    };

    let home = home_dir()?;
    let skill_dir = home.join(skill_dir_rel);
    let _ = std::fs::create_dir_all(&skill_dir);

    let skill_content = include_str!("../../skills/vibearound/SKILL.md");
    let target = skill_dir.join("SKILL.md");

    std::fs::write(&target, skill_content)
        .with_context(|| format!("Write {:?}", target))?;

    eprintln!("[integrations] Installed {} skill at {:?}", agent, target);
    Ok(())
}

/// Remove the vibearound skill directory for a given agent.
fn uninstall_skill(agent: &str) -> anyhow::Result<()> {
    let agent_def = match resources::agent_by_id(agent) {
        Some(def) => def,
        None => return Ok(()),
    };
    let skill_dir_rel = match &agent_def.global_config {
        Some(cfg) => match &cfg.skill_dir {
            Some(dir) => dir,
            None => return Ok(()),
        },
        None => return Ok(()),
    };

    let home = home_dir()?;
    let skill_dir = home.join(skill_dir_rel);
    if skill_dir.exists() {
        let _ = std::fs::remove_dir_all(&skill_dir);
        eprintln!("[integrations] Removed {} skill at {:?}", agent, skill_dir);
    }
    Ok(())
}
