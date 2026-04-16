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
        Some(auth) => format!(
            "http://127.0.0.1:{}/va/mcp?token={}",
            port, auth.token
        ),
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
                eprintln!("[integrations] MCP config uninstall for {}: {:#}", agent, e);
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
    auto_install_npm_agent_with_output(npm_package).await.map(|_| ())
}

/// Like `auto_install_npm_agent` but returns captured stdout/stderr.
pub async fn auto_install_npm_agent_with_output(npm_package: &str) -> anyhow::Result<InstallOutput> {
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
    auto_install_agent_cmd_with_output(install_cmd, agent).await.map(|_| ())
}

/// Like `auto_install_agent_cmd` but returns captured stdout/stderr.
pub async fn auto_install_agent_cmd_with_output(install_cmd: &str, agent: &str) -> anyhow::Result<InstallOutput> {
    eprintln!("[integrations] running install for {}: {}", agent, install_cmd);

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

// ---------------------------------------------------------------------------
// Private — MCP config install/uninstall
// ---------------------------------------------------------------------------

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

fn home_dir() -> anyhow::Result<PathBuf> {
    let dir = config::home_dir();
    if dir.as_os_str() == "/tmp" {
        anyhow::bail!("Cannot determine home directory");
    }
    Ok(dir)
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

/// All skills to deploy, per agent. Returns (skill_name, content) pairs.
/// `skill_name` is used to derive both the target directory and filename.
///
/// Each agent gets the same set of skills; only the directory (and thus the
/// embedded content) differs. The macro eliminates 7× repetition of the
/// skill-name list.
fn agent_skills(agent: &str) -> Vec<(&'static str, &'static str)> {
    macro_rules! skills_for {
        ($dir:literal) => {
            vec![
                ("vibearound",  include_str!(concat!("../../skills/", $dir, "/vibearound/SKILL.md"))),
                ("va-session",  include_str!(concat!("../../skills/", $dir, "/va-session/SKILL.md"))),
                ("va-preview",  include_str!(concat!("../../skills/", $dir, "/va-preview/SKILL.md"))),
                ("va-md-preview", include_str!(concat!("../../skills/", $dir, "/va-md-preview/SKILL.md"))),
            ]
        };
    }

    match agent {
        "claude"    => skills_for!("claude"),
        "gemini"    => skills_for!("gemini"),
        "codex"     => skills_for!("codex"),
        "cursor"    => skills_for!("cursor"),
        "kiro"      => skills_for!("kiro"),
        "qwen-code" => skills_for!("qwen-code"),
        // Generic fallback — top-level skills dir (no agent subdirectory).
        _ => vec![
            ("vibearound",    include_str!("../../skills/vibearound/SKILL.md")),
            ("va-session",    include_str!("../../skills/va-session/SKILL.md")),
            ("va-preview",    include_str!("../../skills/va-preview/SKILL.md")),
            ("va-md-preview", include_str!("../../skills/va-md-preview/SKILL.md")),
        ],
    }
}

/// Install all skill files for a given agent.
fn install_skill(agent: &str) -> anyhow::Result<()> {
    let agent_def = match resources::agent_by_id(agent) {
        Some(def) => def,
        None => return Ok(()),
    };
    let global_config = match &agent_def.global_config {
        Some(cfg) => cfg,
        None => return Ok(()),
    };
    let skill_dir_rel = match &global_config.skill_dir {
        Some(dir) => dir,
        None => return Ok(()),
    };

    let home = home_dir()?;
    let primary_skill_dir = home.join(skill_dir_rel);

    // Derive the parent directory for skill deployment.
    // e.g. ".claude/skills/vibearound" → ".claude/skills"
    // For agents with skill_filename (shared dirs like .cursor/rules/),
    // the skill_dir IS the parent.
    let has_skill_filename = global_config.skill_filename.is_some();
    let skill_base = if has_skill_filename {
        primary_skill_dir.clone()
    } else {
        primary_skill_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or(primary_skill_dir.clone())
    };

    for (skill_name, content) in agent_skills(agent) {
        if has_skill_filename {
            // Shared directory (e.g. .cursor/rules/) — use skill-specific filename
            let ext = global_config
                .skill_filename
                .as_deref()
                .and_then(|f| f.rsplit('.').next())
                .unwrap_or("md");
            let filename = format!("{}.{}", skill_name, ext);
            let target = skill_base.join(&filename);
            let _ = std::fs::create_dir_all(&skill_base);
            std::fs::write(&target, content)
                .with_context(|| format!("Write {:?}", target))?;
            eprintln!("[integrations] Installed {}/{} skill at {:?}", agent, skill_name, target);
        } else {
            // Dedicated directory per skill (e.g. .claude/skills/vibearound/)
            let skill_dir = skill_base.join(skill_name);
            let target = skill_dir.join("SKILL.md");
            let _ = std::fs::create_dir_all(&skill_dir);
            std::fs::write(&target, content)
                .with_context(|| format!("Write {:?}", target))?;
            eprintln!("[integrations] Installed {}/{} skill at {:?}", agent, skill_name, target);
        }
    }
    Ok(())
}

/// Remove all skill files for a given agent.
/// If `skill_filename` is set, removes only skill-specific files (shared directories
/// like `.cursor/rules/` may contain other user files).
/// Otherwise, removes each skill's dedicated directory.
fn uninstall_skill(agent: &str) -> anyhow::Result<()> {
    let agent_def = match resources::agent_by_id(agent) {
        Some(def) => def,
        None => return Ok(()),
    };
    let global_config = match &agent_def.global_config {
        Some(cfg) => cfg,
        None => return Ok(()),
    };
    let skill_dir_rel = match &global_config.skill_dir {
        Some(dir) => dir,
        None => return Ok(()),
    };

    let home = home_dir()?;
    let primary_skill_dir = home.join(skill_dir_rel);
    let has_skill_filename = global_config.skill_filename.is_some();
    let skill_base = if has_skill_filename {
        primary_skill_dir.clone()
    } else {
        primary_skill_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or(primary_skill_dir.clone())
    };

    for (skill_name, _) in agent_skills(agent) {
        if has_skill_filename {
            let ext = global_config
                .skill_filename
                .as_deref()
                .and_then(|f| f.rsplit('.').next())
                .unwrap_or("md");
            let filename = format!("{}.{}", skill_name, ext);
            let target = skill_base.join(&filename);
            if target.exists() {
                let _ = std::fs::remove_file(&target);
                eprintln!("[integrations] Removed {}/{} skill at {:?}", agent, skill_name, target);
            }
        } else {
            let skill_dir = skill_base.join(skill_name);
            if skill_dir.exists() {
                let _ = std::fs::remove_dir_all(&skill_dir);
                eprintln!("[integrations] Removed {}/{} skill at {:?}", agent, skill_name, skill_dir);
            }
        }
    }
    Ok(())
}
