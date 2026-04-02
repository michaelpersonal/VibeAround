//! Environment utilities for distributed desktop app.
//!
//! When launched from Finder / Start Menu, the process inherits a minimal PATH
//! that lacks user-installed tools (node, npx, gemini, etc.).  This module
//! probes the user's login shell (Unix) or well-known install locations (Windows)
//! once at startup, caches the result, and exposes helpers that create child
//! process Commands with the enriched PATH.

use std::sync::OnceLock;

static ENRICHED_PATH: OnceLock<String> = OnceLock::new();

/// Return the enriched PATH string.  Probed once on first call, cached forever.
pub fn enriched_path() -> &'static str {
    ENRICHED_PATH.get_or_init(|| {
        let result = probe_enriched_path();
        eprintln!("[env] enriched PATH ({} entries)", result.matches(':').count() + 1);
        result
    })
}

/// Create a `tokio::process::Command` with the enriched PATH pre-set.
/// Drop-in replacement for `tokio::process::Command::new(program)`.
pub fn command(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.env("PATH", enriched_path());
    cmd
}

/// Create a `std::process::Command` with the enriched PATH pre-set.
pub fn std_command(program: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.env("PATH", enriched_path());
    cmd
}

/// Directory where npm-based ACP agent packages are installed.
/// Shared with channel plugins at `~/.vibearound/plugins/` so common
/// dependencies (e.g. `@agentclientprotocol/sdk`, `zod`) are deduped.
pub fn acp_agents_dir() -> std::path::PathBuf {
    crate::config::data_dir().join("plugins")
}

/// Resolve the JS entry point for a pre-installed npm ACP agent binary.
///
/// Looks up `~/.vibearound/plugins/node_modules/.bin/<bin_name>`.
/// On Unix the `.bin/` entries are symlinks to the actual JS file — we
/// follow the symlink.  On Windows npm creates `.cmd` wrappers; we parse
/// them to extract the JS path.
pub fn resolve_acp_agent_bin(bin_name: &str) -> anyhow::Result<std::path::PathBuf> {
    let bin_dir = acp_agents_dir().join("node_modules").join(".bin");
    let bin_path = bin_dir.join(bin_name);

    #[cfg(unix)]
    {
        if !bin_path.exists() {
            anyhow::bail!(
                "ACP agent binary '{}' not found at {:?}. Run onboarding to install it.",
                bin_name,
                bin_path
            );
        }
        // Follow symlink to actual JS file
        let resolved = std::fs::canonicalize(&bin_path)
            .map_err(|e| anyhow::anyhow!("cannot resolve symlink {:?}: {}", bin_path, e))?;
        Ok(resolved)
    }

    #[cfg(windows)]
    {
        // On Windows, npm creates <name>.cmd; parse it to find the JS entry
        let cmd_path = bin_dir.join(format!("{}.cmd", bin_name));
        if !cmd_path.exists() {
            anyhow::bail!(
                "ACP agent binary '{}' not found at {:?}. Run onboarding to install it.",
                bin_name,
                cmd_path
            );
        }
        // .cmd files contain a line like: @node "path\to\script.js" %*
        let content = std::fs::read_to_string(&cmd_path)?;
        for line in content.lines() {
            let trimmed = line.trim().trim_start_matches('@');
            // Look for: node "..." or node ...
            if let Some(rest) = trimmed.strip_prefix("node ").or_else(|| trimmed.strip_prefix("node.exe ")) {
                let js_path = rest
                    .trim()
                    .trim_matches('"')
                    .trim_end_matches(" %*")
                    .trim_end_matches(" %~dp0")
                    .trim_matches('"');
                let resolved = bin_dir.join(js_path);
                if resolved.exists() {
                    return Ok(std::fs::canonicalize(&resolved)?);
                }
                // Try as absolute path
                let abs = std::path::PathBuf::from(js_path);
                if abs.exists() {
                    return Ok(std::fs::canonicalize(&abs)?);
                }
            }
        }
        anyhow::bail!("could not parse JS entry from {:?}", cmd_path);
    }
}

// ---------------------------------------------------------------------------
// Platform-specific PATH probing
// ---------------------------------------------------------------------------

fn probe_enriched_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();

    #[cfg(unix)]
    {
        if let Some(shell_path) = probe_unix_login_shell() {
            return shell_path;
        }
        // Fallback: append well-known Unix paths
        let extras = [
            "/opt/homebrew/bin",
            "/usr/local/bin",
        ];
        let mut parts: Vec<&str> = current.split(':').collect();
        for extra in &extras {
            if !parts.contains(extra) && std::path::Path::new(extra).is_dir() {
                parts.push(extra);
            }
        }
        // Probe NVM default
        if let Ok(home) = std::env::var("HOME") {
            let nvm_default = format!("{}/.nvm/alias/default", home);
            if let Ok(version_alias) = std::fs::read_to_string(&nvm_default) {
                let version = version_alias.trim();
                // Find matching directory
                let nvm_versions = format!("{}/.nvm/versions/node", home);
                if let Ok(entries) = std::fs::read_dir(&nvm_versions) {
                    for entry in entries.flatten() {
                        let name = entry.file_name();
                        let name_str = name.to_string_lossy();
                        if name_str.starts_with(version) || name_str.contains(version) {
                            let bin = entry.path().join("bin");
                            if bin.is_dir() {
                                let bin_str = bin.to_string_lossy().to_string();
                                if !current.contains(&bin_str) {
                                    parts.insert(0, Box::leak(bin_str.into_boxed_str()));
                                }
                            }
                        }
                    }
                }
            }
        }
        return parts.join(":");
    }

    #[cfg(windows)]
    {
        let sep = ";";
        let mut parts: Vec<String> = current.split(sep).map(String::from).collect();
        let candidates: Vec<String> = vec![
            std::env::var("APPDATA").map(|d| format!("{}\\npm", d)).unwrap_or_default(),
            std::env::var("ProgramFiles").map(|d| format!("{}\\nodejs", d)).unwrap_or_default(),
            std::env::var("LOCALAPPDATA").map(|d| format!("{}\\Volta\\bin", d)).unwrap_or_default(),
        ];
        for candidate in candidates {
            if !candidate.is_empty()
                && !parts.iter().any(|p| p.eq_ignore_ascii_case(&candidate))
                && std::path::Path::new(&candidate).is_dir()
            {
                parts.push(candidate);
            }
        }
        return parts.join(sep);
    }
}

/// Probe the user's login shell for their full PATH.
#[cfg(unix)]
fn probe_unix_login_shell() -> Option<String> {
    let shells_to_try: Vec<String> = {
        let mut shells = Vec::new();
        if let Ok(user_shell) = std::env::var("SHELL") {
            shells.push(user_shell);
        }
        shells.push("/bin/zsh".to_string());
        shells.push("/bin/bash".to_string());
        shells
    };

    for shell in &shells_to_try {
        if !std::path::Path::new(shell).exists() {
            continue;
        }
        let result = std::process::Command::new(shell)
            .args(["-lc", "echo $PATH"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        match result {
            Ok(output) => {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && path.contains('/') {
                    eprintln!("[env] probed PATH from {}", shell);
                    return Some(path);
                }
            }
            Err(e) => {
                eprintln!("[env] failed to probe {}: {}", shell, e);
            }
        }
    }
    None
}
