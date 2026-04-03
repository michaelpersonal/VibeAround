//! Agent integration management — MCP config, skill files, and ACP agent npm packages.
//!
//! Syncs VibeAround integrations into each coding agent's global settings.
//! Identifies managed entries by the "vibearound" key name in MCP server
//! configs and the "vibearound" skill directory name.

use std::path::PathBuf;

use anyhow::{anyhow, Context};

use crate::{config, resources};


// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Sync all agent integrations with the current settings.
/// - Enabled agents: install/update MCP config + skills.
/// - Disabled agents: remove MCP config + skills.
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
// Private — MCP config install/uninstall
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

/// Check if the agent uses TOML config format.
fn is_toml_format(global_config: &resources::AgentGlobalConfig) -> bool {
    global_config
        .settings_format
        .as_deref()
        == Some("toml")
}

/// Merge VibeAround MCP server entry into an agent's global settings.
/// Supports JSON (default) and TOML formats. Also writes to legacy path if configured.
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

    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if is_toml_format(global_config) {
        install_mcp_config_toml(&config_path, &global_config.mcp_key, &global_config.mcp_entry, mcp_url, agent)?;
    } else {
        install_mcp_config_json(&config_path, &global_config.mcp_key, &global_config.mcp_entry, mcp_url, agent)?;
    }

    // Also write to legacy path for backward compat (e.g. older Claude Code versions)
    if let Some(legacy) = &global_config.settings_path_legacy {
        let legacy_path = home.join(legacy);
        if let Some(parent) = legacy_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = install_mcp_config_json(&legacy_path, &global_config.mcp_key, &global_config.mcp_entry, mcp_url, agent);
    }

    Ok(())
}

fn install_mcp_config_json(
    config_path: &std::path::Path,
    mcp_key: &str,
    mcp_entry_template: &serde_json::Value,
    mcp_url: &str,
    agent: &str,
) -> anyhow::Result<()> {
    // Substitute placeholders in the entry template
    let mcp_value_str = serde_json::to_string(mcp_entry_template)
        .context("serialize mcp_entry")?;
    let mcp_value: serde_json::Value = serde_json::from_str(
        &mcp_value_str
            .replace("{mcp_url}", mcp_url),
    )
    .context("parse mcp_entry after substitution")?;

    // Read existing config
    let data = std::fs::read_to_string(config_path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value =
        serde_json::from_str(&data).unwrap_or(serde_json::json!({}));

    // Always replace (full replace on every startup)
    if let Some(obj) = root.as_object_mut() {
        let servers = obj
            .entry(mcp_key)
            .or_insert_with(|| serde_json::json!({}));
        if let Some(servers_obj) = servers.as_object_mut() {
            servers_obj.insert("vibearound".to_string(), mcp_value);
        }
    }

    let pretty = serde_json::to_string_pretty(&root).context("JSON serialize")?;
    std::fs::write(config_path, pretty)
        .with_context(|| format!("Write {:?}", config_path))?;

    eprintln!(
        "[integrations] Installed MCP config for {} at {:?}",
        agent, config_path
    );
    Ok(())
}

fn install_mcp_config_toml(
    config_path: &std::path::Path,
    mcp_key: &str,
    mcp_entry_template: &serde_json::Value,
    mcp_url: &str,
    agent: &str,
) -> anyhow::Result<()> {
    use toml_edit::{DocumentMut, Item, Table};

    // Substitute placeholders in the entry template
    let mcp_value_str = serde_json::to_string(mcp_entry_template)
        .context("serialize mcp_entry")?;
    let substituted = mcp_value_str
        .replace("{mcp_url}", mcp_url);
    let mcp_value: serde_json::Value = serde_json::from_str(&substituted)
        .context("parse mcp_entry after substitution")?;

    // Read existing TOML config
    let data = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc: DocumentMut = data.parse::<DocumentMut>().unwrap_or_default();

    // Ensure [mcp_key] table exists (e.g. [mcp_servers])
    if !doc.contains_key(mcp_key) {
        doc[mcp_key] = Item::Table(Table::new());
    }

    // Create the [mcp_key.vibearound] sub-table
    let servers = doc[mcp_key].as_table_mut()
        .ok_or_else(|| anyhow!("{} is not a table in {:?}", mcp_key, config_path))?;

    let mut entry_table = Table::new();
    if let Some(obj) = mcp_value.as_object() {
        for (k, v) in obj {
            match v {
                serde_json::Value::String(s) => {
                    entry_table[k.as_str()] = toml_edit::value(s.as_str());
                }
                serde_json::Value::Bool(b) => {
                    entry_table[k.as_str()] = toml_edit::value(*b);
                }
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        entry_table[k.as_str()] = toml_edit::value(i);
                    } else if let Some(f) = n.as_f64() {
                        entry_table[k.as_str()] = toml_edit::value(f);
                    }
                }
                _ => {} // skip complex values
            }
        }
    }

    servers["vibearound"] = Item::Table(entry_table);

    std::fs::write(config_path, doc.to_string())
        .with_context(|| format!("Write {:?}", config_path))?;

    eprintln!(
        "[integrations] Installed MCP config for {} at {:?} (TOML)",
        agent, config_path
    );
    Ok(())
}

/// Remove VibeAround MCP server entry from an agent's global settings.
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

    if is_toml_format(global_config) {
        return uninstall_mcp_config_toml(&config_path, mcp_key, agent);
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

fn uninstall_mcp_config_toml(
    config_path: &std::path::Path,
    mcp_key: &str,
    agent: &str,
) -> anyhow::Result<()> {
    use toml_edit::DocumentMut;

    let data = std::fs::read_to_string(config_path)
        .with_context(|| format!("Read {:?}", config_path))?;
    let mut doc: DocumentMut = data.parse::<DocumentMut>()
        .with_context(|| format!("Parse TOML {:?}", config_path))?;

    let mut changed = false;
    if let Some(servers) = doc.get_mut(mcp_key).and_then(|v| v.as_table_mut()) {
        if servers.remove("vibearound").is_some() {
            changed = true;
        }
    }

    if changed {
        std::fs::write(config_path, doc.to_string())
            .with_context(|| format!("Write {:?}", config_path))?;
        eprintln!(
            "[integrations] Removed MCP config for {} at {:?} (TOML)",
            agent, config_path
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Private — skill file install/uninstall
// ---------------------------------------------------------------------------

/// Per-agent skill content, embedded at compile time.
fn agent_skill_content(agent: &str) -> &'static str {
    match agent {
        "claude" => include_str!("../../skills/claude/vibearound/SKILL.md"),
        "gemini" => include_str!("../../skills/gemini/vibearound/SKILL.md"),
        "codex" => include_str!("../../skills/codex/vibearound/SKILL.md"),
        _ => include_str!("../../skills/vibearound/SKILL.md"),
    }
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
    let target = skill_dir.join("SKILL.md");

    // Always replace (full replace on every startup)
    let _ = std::fs::create_dir_all(&skill_dir);
    std::fs::write(&target, agent_skill_content(agent))
        .with_context(|| format!("Write {:?}", target))?;

    eprintln!(
        "[integrations] Installed {} skill at {:?}",
        agent, target
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
