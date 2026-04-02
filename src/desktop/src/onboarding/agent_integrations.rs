//! Agent integration installation — MCP config + skill files.
//! Syncs VibeAround config into each coding agent's global settings directory.

use std::path::PathBuf;

use anyhow::{anyhow, Context};

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

/// Sync VibeAround integrations with coding agents' global settings.
/// Installs MCP config + skills for enabled agents, removes them for disabled ones.
/// Errors are non-fatal and logged per-agent; the function always returns Ok.
pub(super) fn install_agent_integrations(settings: &serde_json::Value) -> anyhow::Result<()> {
    let port = common::config::DEFAULT_PORT;
    let mcp_url = format!("http://127.0.0.1:{}/mcp", port);

    let all_agents = common::resources::agent_ids();
    let enabled_agents: Vec<String> = settings
        .get("enabled_agents")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| all_agents.iter().map(|s| s.to_string()).collect());

    for agent in &all_agents {
        let enabled = enabled_agents.iter().any(|a| a == agent);
        if enabled {
            if let Err(e) = install_mcp_config(agent, &mcp_url) {
                eprintln!("[onboarding] MCP config install for {}: {:#}", agent, e);
            }
            if let Err(e) = install_agent_skill(agent) {
                eprintln!("[onboarding] skill install for {}: {:#}", agent, e);
            }
        } else {
            if let Err(e) = uninstall_mcp_config(agent) {
                eprintln!("[onboarding] MCP config uninstall for {}: {:#}", agent, e);
            }
            if let Err(e) = uninstall_agent_skill(agent) {
                eprintln!("[onboarding] skill uninstall for {}: {:#}", agent, e);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Get the user's home directory, cross-platform.
fn home_dir() -> anyhow::Result<PathBuf> {
    // Try HOME (macOS/Linux), then USERPROFILE (Windows)
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| anyhow!("Cannot determine home directory"))
}

/// Merge VibeAround MCP server entry into an agent's global settings JSON file.
fn install_mcp_config(agent: &str, mcp_url: &str) -> anyhow::Result<()> {
    let home = home_dir()?;

    let agent_def = match common::resources::agent_by_id(agent) {
        Some(def) => def,
        None => return Ok(()),
    };
    let global_config = match &agent_def.global_config {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let config_path = home.join(&global_config.settings_path);
    let mcp_key = &global_config.mcp_key;

    // Replace {mcp_url} placeholder in the entry template
    let mcp_value_str = serde_json::to_string(&global_config.mcp_entry)
        .context("serialize mcp_entry")?;
    let mcp_value: serde_json::Value =
        serde_json::from_str(&mcp_value_str.replace("{mcp_url}", mcp_url))
            .context("parse mcp_entry after substitution")?;

    // Read existing config or start fresh
    let data = std::fs::read_to_string(&config_path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value =
        serde_json::from_str(&data).unwrap_or(serde_json::json!({}));

    // Merge: add vibearound entry under mcpServers, don't touch other keys
    if let Some(obj) = root.as_object_mut() {
        let servers = obj
            .entry(mcp_key)
            .or_insert_with(|| serde_json::json!({}));
        if let Some(servers_obj) = servers.as_object_mut() {
            servers_obj.insert("vibearound".to_string(), mcp_value);
        }
    }

    // Write back
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let pretty = serde_json::to_string_pretty(&root).context("JSON serialize")?;
    std::fs::write(&config_path, pretty)
        .with_context(|| format!("Write {:?}", config_path))?;

    eprintln!(
        "[onboarding] Installed VibeAround MCP config for {} at {:?}",
        agent, config_path
    );
    Ok(())
}

/// Remove VibeAround MCP server entry from an agent's global settings JSON file.
fn uninstall_mcp_config(agent: &str) -> anyhow::Result<()> {
    let home = home_dir()?;

    let agent_def = match common::resources::agent_by_id(agent) {
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

    // Remove vibearound entry from mcpServers
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
            "[onboarding] Removed VibeAround MCP config for {} at {:?}",
            agent, config_path
        );
    }

    Ok(())
}

/// Install the vibearound skill file for a given agent.
fn install_agent_skill(agent: &str) -> anyhow::Result<()> {
    let agent_def = match common::resources::agent_by_id(agent) {
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

    let skill_content = include_str!("../../../skills/vibearound/SKILL.md");
    let target = skill_dir.join("SKILL.md");

    std::fs::write(&target, skill_content)
        .with_context(|| format!("Write {:?}", target))?;

    eprintln!("[onboarding] Installed {} skill at {:?}", agent, target);
    Ok(())
}

/// Remove the vibearound skill directory for a given agent.
fn uninstall_agent_skill(agent: &str) -> anyhow::Result<()> {
    let agent_def = match common::resources::agent_by_id(agent) {
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
        eprintln!("[onboarding] Removed {} skill at {:?}", agent, skill_dir);
    }
    Ok(())
}
