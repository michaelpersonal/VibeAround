//! MCP server entry install/uninstall for an agent's global settings.
//!
//! Supports both JSON (Claude Code, Gemini, Cursor, Kiro, Qwen) and TOML
//! (Codex) formats. Also writes to any configured `settings_path_legacy`
//! path for backward compatibility.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};

use crate::{config, resources};

/// Merge VibeAround MCP server entry into an agent's global settings.
/// Supports JSON (default) and TOML formats. Also writes to legacy path
/// if configured.
pub(super) fn install_mcp_config(agent: &str, mcp_url: &str) -> anyhow::Result<()> {
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
        install_mcp_config_toml(
            &config_path,
            &global_config.mcp_key,
            &global_config.mcp_entry,
            mcp_url,
            agent,
        )?;
    } else {
        install_mcp_config_json(
            &config_path,
            &global_config.mcp_key,
            &global_config.mcp_entry,
            mcp_url,
            agent,
        )?;
    }

    // Also write to legacy path for backward compat (e.g. older Claude Code versions)
    if let Some(legacy) = &global_config.settings_path_legacy {
        let legacy_path = home.join(legacy);
        if let Some(parent) = legacy_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = install_mcp_config_json(
            &legacy_path,
            &global_config.mcp_key,
            &global_config.mcp_entry,
            mcp_url,
            agent,
        );
    }

    Ok(())
}

/// Remove VibeAround MCP server entry from an agent's global settings.
pub(super) fn uninstall_mcp_config(agent: &str) -> anyhow::Result<()> {
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

/// Check if the agent uses TOML config format.
fn is_toml_format(global_config: &resources::AgentGlobalConfig) -> bool {
    global_config.settings_format.as_deref() == Some("toml")
}

pub(super) fn home_dir() -> anyhow::Result<PathBuf> {
    let dir = config::home_dir();
    if dir.as_os_str() == "/tmp" {
        anyhow::bail!("Cannot determine home directory");
    }
    Ok(dir)
}

fn install_mcp_config_json(
    config_path: &Path,
    mcp_key: &str,
    mcp_entry_template: &serde_json::Value,
    mcp_url: &str,
    agent: &str,
) -> anyhow::Result<()> {
    // Substitute placeholders in the entry template
    let mcp_value_str =
        serde_json::to_string(mcp_entry_template).context("serialize mcp_entry")?;
    let mcp_value: serde_json::Value =
        serde_json::from_str(&mcp_value_str.replace("{mcp_url}", mcp_url))
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
    config_path: &Path,
    mcp_key: &str,
    mcp_entry_template: &serde_json::Value,
    mcp_url: &str,
    agent: &str,
) -> anyhow::Result<()> {
    use toml_edit::{DocumentMut, Item, Table};

    // Substitute placeholders in the entry template
    let mcp_value_str =
        serde_json::to_string(mcp_entry_template).context("serialize mcp_entry")?;
    let substituted = mcp_value_str.replace("{mcp_url}", mcp_url);
    let mcp_value: serde_json::Value =
        serde_json::from_str(&substituted).context("parse mcp_entry after substitution")?;

    // Read existing TOML config
    let data = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc: DocumentMut = data.parse::<DocumentMut>().unwrap_or_default();

    // Ensure [mcp_key] table exists (e.g. [mcp_servers])
    if !doc.contains_key(mcp_key) {
        doc[mcp_key] = Item::Table(Table::new());
    }

    // Create the [mcp_key.vibearound] sub-table
    let servers = doc[mcp_key]
        .as_table_mut()
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

fn uninstall_mcp_config_toml(
    config_path: &Path,
    mcp_key: &str,
    agent: &str,
) -> anyhow::Result<()> {
    use toml_edit::DocumentMut;

    let data = std::fs::read_to_string(config_path)
        .with_context(|| format!("Read {:?}", config_path))?;
    let mut doc: DocumentMut = data
        .parse::<DocumentMut>()
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
