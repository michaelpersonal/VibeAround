//! Agent integration management — MCP config, skill files, and ACP agent npm packages.
//!
//! Syncs VibeAround integrations into each coding agent's global settings.
//! Uses a `_vibearound` metadata block (in both SKILL.md frontmatter and MCP
//! JSON entries) for version tracking and cleanup of stale entries.

use std::path::PathBuf;

use anyhow::{anyhow, Context};

use crate::{config, resources};

/// App version, used as the integration version stamp.
const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Sync all agent integrations with the current settings.
/// - Enabled agents: install/update MCP config + skills (version-checked).
/// - Disabled agents: remove MCP config + skills.
/// - Stale entries (from previous versions with different paths/keys): cleaned up.
pub fn sync_integrations(settings: &serde_json::Value) {
    let port = config::DEFAULT_PORT;
    let mcp_url = format!("http://127.0.0.1:{}/mcp", port);

    let all_agents = resources::agent_ids();
    let enabled_agents = resolve_enabled_agents(settings, &all_agents);

    // First pass: clean up any stale vibearound-managed entries in all agent configs
    for agent in &all_agents {
        if let Err(e) = cleanup_stale_mcp_entries(agent, &mcp_url) {
            eprintln!("[integrations] stale MCP cleanup for {}: {:#}", agent, e);
        }
    }

    // Second pass: install for enabled, uninstall for disabled
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
pub async fn auto_install_npm_agent(npm_package: &str) -> anyhow::Result<()> {
    let plugins_dir = crate::env::acp_agents_dir();
    std::fs::create_dir_all(&plugins_dir)
        .with_context(|| format!("creating {:?}", plugins_dir))?;

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
// Private — version-aware MCP config
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

/// Metadata string format: "vibearound <version>"
const METADATA_PREFIX: &str = "vibearound";

/// Check if a JSON value has `_metadata` starting with "vibearound".
fn is_vibearound_managed(value: &serde_json::Value) -> bool {
    value
        .get("metadata")
        .and_then(|v| v.as_str())
        .map(|s| s.starts_with(METADATA_PREFIX))
        .unwrap_or(false)
}

/// Get the metadata string (e.g. "vibearound 0.0.1") from a JSON value.
fn get_metadata(value: &serde_json::Value) -> Option<String> {
    value.get("metadata").and_then(|v| v.as_str()).map(String::from)
}

/// The metadata string for the current version.
fn current_metadata() -> String {
    format!("{} {}", METADATA_PREFIX, VERSION)
}

/// Remove any vibearound-managed MCP entries that don't match the current key.
/// This handles renames (e.g. "vibearound" → "va" or vice versa).
fn cleanup_stale_mcp_entries(agent: &str, _mcp_url: &str) -> anyhow::Result<()> {
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
    if !config_path.exists() {
        return Ok(());
    }

    let data = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Read {:?}", config_path))?;
    let mut root: serde_json::Value =
        serde_json::from_str(&data).unwrap_or(serde_json::json!({}));

    let mcp_key = &global_config.mcp_key;
    let mut changed = false;

    if let Some(obj) = root.as_object_mut() {
        if let Some(servers) = obj.get_mut(mcp_key) {
            if let Some(servers_obj) = servers.as_object_mut() {
                // Find and remove any managed entries that aren't "vibearound"
                let stale_keys: Vec<String> = servers_obj
                    .iter()
                    .filter(|(key, val)| *key != "vibearound" && is_vibearound_managed(val))
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in stale_keys {
                    servers_obj.remove(&key);
                    changed = true;
                    eprintln!(
                        "[integrations] Removed stale MCP entry '{}' for {} at {:?}",
                        key, agent, config_path
                    );
                }
            }
        }
    }

    if changed {
        let pretty = serde_json::to_string_pretty(&root).context("JSON serialize")?;
        std::fs::write(&config_path, pretty)
            .with_context(|| format!("Write {:?}", config_path))?;
    }

    Ok(())
}

/// Merge VibeAround MCP server entry into an agent's global settings JSON.
/// Skips the write if the installed version matches the current app version.
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

    // Substitute placeholders in the entry template
    let mcp_value_str = serde_json::to_string(&global_config.mcp_entry)
        .context("serialize mcp_entry")?;
    let mcp_value: serde_json::Value = serde_json::from_str(
        &mcp_value_str
            .replace("{mcp_url}", mcp_url)
            .replace("{version}", VERSION),
    )
    .context("parse mcp_entry after substitution")?;

    // Read existing config
    let data = std::fs::read_to_string(&config_path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value =
        serde_json::from_str(&data).unwrap_or(serde_json::json!({}));

    // Check if already installed with current version
    if let Some(existing) = root.get(mcp_key).and_then(|s| s.get("vibearound")) {
        if get_metadata(existing).as_deref() == Some(&current_metadata()) {
            return Ok(()); // already up-to-date
        }
    }

    // Install / update
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
        "[integrations] Installed MCP config v{} for {} at {:?}",
        VERSION, agent, config_path
    );
    Ok(())
}

/// Remove VibeAround MCP server entry from an agent's global settings JSON.
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
                // Remove any vibearound-managed entries
                let managed_keys: Vec<String> = servers_obj
                    .iter()
                    .filter(|(_, val)| is_vibearound_managed(val))
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in managed_keys {
                    servers_obj.remove(&key);
                    changed = true;
                }
                // Also remove the "vibearound" key specifically (legacy, may not have metadata)
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

// ---------------------------------------------------------------------------
// Private — version-aware skill files
// ---------------------------------------------------------------------------

/// Install the vibearound skill file. Skips write if version matches.
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
    let target = skill_dir.join("SKILL.md");

    // Check installed version via frontmatter metadata
    if target.exists() {
        if let Ok(content) = std::fs::read_to_string(&target) {
            if content.contains(&current_metadata()) {
                return Ok(()); // already up-to-date
            }
        }
    }

    // Install with version substitution
    let _ = std::fs::create_dir_all(&skill_dir);
    let skill_content = include_str!("../../skills/vibearound/SKILL.md")
        .replace("${VERSION}", VERSION);
    std::fs::write(&target, skill_content)
        .with_context(|| format!("Write {:?}", target))?;

    eprintln!(
        "[integrations] Installed {} skill v{} at {:?}",
        agent, VERSION, target
    );
    Ok(())
}

/// Remove the vibearound skill directory for a given agent.
/// Scans for any directories containing vibearound-managed SKILL.md files.
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
