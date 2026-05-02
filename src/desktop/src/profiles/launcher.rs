//! Terminal launcher — write a one-shot bash script with the rendered env
//! exports and `exec` into the right CLI, then ask Terminal.app to open it.
//!
//! VibeAround does not track the spawned terminal: once the user has the
//! window, the CLI is theirs. This module is fire-and-forget by design.
//!
//! # Security
//!
//! - All env values + the cwd path are run through `shell_escape::unix::escape`
//!   before being interpolated into the bash script. The script itself is
//!   written 0600 and self-deletes on its first line so a `cat` between
//!   spawn and exec is a narrow race window.
//! - The script path passed to AppleScript is escaped against AppleScript
//!   string semantics (a separate ruleset from POSIX shell quoting); the
//!   two are not interchangeable.

use std::collections::HashMap;
use std::path::Path;
#[cfg(target_os = "windows")]
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context};

#[cfg(target_os = "windows")]
use common::auth;
use common::{config, profiles, resources};
use toml_edit::{value, DocumentMut};

use super::terminal::{self, TerminalChoice};
use profiles::ProfileDef;

// ---------------------------------------------------------------------------
// Public entry
// ---------------------------------------------------------------------------

pub fn launch(profile: &ProfileDef, launch_target: &str) -> anyhow::Result<()> {
    let mut rendered = profiles::runtime::render_for_launch(profile, launch_target)?;
    apply_compatibility_proxy(profile, launch_target, &mut rendered)?;
    do_launch(profile, launch_target, rendered)
}

/// "Direct" launch — open a Terminal running the named coding CLI with NO
/// env at all. The CLI uses whatever global OAuth / login / config it has
/// on disk (`~/.claude/`, `~/.codex/`, `~/.gemini/`, etc.), so VibeAround
/// stays out of the credential business entirely. `agent_id` matches the
/// `id` field in `agents.json`.
pub fn launch_direct(agent_id: &str) -> anyhow::Result<()> {
    let agent = resources::agent_by_id(agent_id)
        .ok_or_else(|| anyhow!("agent '{}' not found in agents.json", agent_id))?;
    let workspace = terminal::resolve_workspace_preference()?;
    spawn_terminal(
        &[],
        &agent.pty.command,
        &format!("{} (direct)", agent.display_name),
        &workspace,
    )
}

fn do_launch(
    profile: &ProfileDef,
    launch_target: &str,
    rendered: profiles::render::RenderedProfile,
) -> anyhow::Result<()> {
    let command_args = rendered.command_args.clone();
    let env = profiles::runtime::materialize_env(&profile.id, rendered)?;
    let agent_id = profiles::runtime::agent_id_for(launch_target)?;
    let agent = resources::agent_by_id(agent_id)
        .ok_or_else(|| anyhow!("agent '{}' not found in agents.json", agent_id))?;
    let pty_command = command_with_args(&agent.pty.command, &command_args);
    let workspace = terminal::resolve_workspace_preference()?;

    spawn_terminal(&env, &pty_command, &profile.label, &workspace)?;
    Ok(())
}

fn command_with_args(command: &str, args: &[String]) -> String {
    if args.is_empty() {
        return command.to_string();
    }

    let mut out = command.to_string();
    for arg in args {
        out.push(' ');
        out.push_str(&shell_escape::unix::escape(std::borrow::Cow::Borrowed(
            arg.as_str(),
        )));
    }
    out
}

fn apply_compatibility_proxy(
    profile: &ProfileDef,
    launch_target: &str,
    rendered: &mut profiles::render::RenderedProfile,
) -> anyhow::Result<()> {
    let mode = terminal::read_compatibility_proxy_preference();
    if mode == terminal::CompatibilityProxyMode::Off {
        return Ok(());
    }

    let provider = profiles::catalog::get(&profile.provider)
        .ok_or_else(|| anyhow!("unknown provider '{}'", profile.provider))?;
    let api_type = profiles::runtime::api_type_for_launch_target(profile, provider, launch_target)?;

    if api_type != "openai-chat" || launch_target != "codex" {
        return Ok(());
    }

    let proxy_base_url = format!(
        "http://127.0.0.1:{}/va/openai-proxy/{}/v1",
        config::DEFAULT_PORT,
        profile.id
    );

    for settings_file in &mut rendered.settings_files {
        if settings_file.rel_path == "config.toml" {
            settings_file.contents = rewrite_codex_config_for_proxy(
                &settings_file.contents,
                &profile.provider,
                &proxy_base_url,
            )?;
        }
    }

    Ok(())
}

fn rewrite_codex_config_for_proxy(
    contents: &str,
    provider_id: &str,
    proxy_base_url: &str,
) -> anyhow::Result<String> {
    let mut doc = contents
        .parse::<DocumentMut>()
        .context("parse generated Codex config.toml")?;
    let provider = doc["model_providers"][provider_id]
        .as_table_mut()
        .ok_or_else(|| {
            anyhow!(
                "generated Codex config has no model_providers.{} table",
                provider_id
            )
        })?;
    provider["base_url"] = value(proxy_base_url);
    provider["wire_api"] = value("responses");
    Ok(doc.to_string())
}

// ---------------------------------------------------------------------------
// macOS Terminal spawn
// ---------------------------------------------------------------------------
//
// We hand the rendered bash script to `open -a <Terminal.app>` rather than
// driving Terminal/iTerm via osascript. `open` goes through LaunchServices,
// which is a normal user-initiated action — no Automation TCC permission
// prompt, no dance through System Settings → Privacy & Security. The
// `.command` extension is the canonical macOS marker for "executable shell
// script that opens a terminal window when launched"; both Terminal.app and
// iTerm2 register as handlers for it.

#[cfg(target_os = "macos")]
fn spawn_terminal(
    env: &[(String, String)],
    command: &str,
    window_label: &str,
    workspace: &Path,
) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let script = build_bash_script(env, command, window_label, workspace);
    let script_path = std::env::temp_dir().join(format!(
        "vibearound-launch-{}.command",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&script_path, &script)
        .with_context(|| format!("write launch script {:?}", script_path))?;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod launch script {:?}", script_path))?;

    let choice = terminal::read_preference();
    let app_name = match choice {
        TerminalChoice::Terminal => "Terminal",
        TerminalChoice::Iterm2 => "iTerm",
        other => bail!("terminal '{}' is not supported on macOS", other.id()),
    };

    let status = std::process::Command::new("open")
        .arg("-a")
        .arg(app_name)
        .arg(&script_path)
        .status()
        .with_context(|| format!("invoke `open -a {}`", app_name))?;

    if !status.success() {
        // The script self-deletes on its first line; only nuke it ourselves
        // if `open` never even launched a terminal that could run it.
        let _ = std::fs::remove_file(&script_path);
        bail!(
            "`open -a {}` failed (exit {:?}). Make sure {0}.app is installed and try again.",
            app_name,
            status.code()
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[cfg(not(target_os = "windows"))]
fn spawn_terminal(
    _env: &[(String, String)],
    _command: &str,
    _window_label: &str,
    _workspace: &Path,
) -> anyhow::Result<()> {
    bail!("Profile launch is only supported on macOS and Windows");
}

// ---------------------------------------------------------------------------
// Windows terminal spawn
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn spawn_terminal(
    env: &[(String, String)],
    command: &str,
    window_label: &str,
    workspace: &Path,
) -> anyhow::Result<()> {
    let choice = terminal::read_preference();
    let script_path = write_windows_launch_script(env, command, window_label, choice, workspace)?;
    let title = format!("VibeAround - {}", window_label);

    let params = match choice {
        TerminalChoice::PowerShell => format!(
            "/C start {} powershell.exe -ExecutionPolicy Bypass -NoExit -File {}",
            quote_cmd_arg(&title),
            quote_cmd_arg(&script_path.to_string_lossy())
        ),
        TerminalChoice::Cmd => format!(
            "/C start {} cmd.exe /K {}",
            quote_cmd_arg(&title),
            quote_cmd_arg(&script_path.to_string_lossy())
        ),
        other => bail!("terminal '{}' is not supported on Windows", other.id()),
    };

    // Use ShellExecuteW through the `open` crate instead of Rust `Command`.
    // `Command` inherits all inheritable handles by default on Windows; if a
    // launched CLI keeps the daemon's TCP listener handle alive, VibeAround's
    // next start sees 127.0.0.1:12358 as occupied by a stale PID.
    open::with(params, "cmd.exe").with_context(|| format!("open {}", choice.label()))?;

    Ok(())
}

#[cfg(target_os = "windows")]
fn write_windows_launch_script(
    env: &[(String, String)],
    command: &str,
    window_label: &str,
    choice: TerminalChoice,
    workspace: &Path,
) -> anyhow::Result<PathBuf> {
    let script_path = match choice {
        TerminalChoice::PowerShell => {
            std::env::temp_dir().join(format!("vibearound-launch-{}.ps1", uuid::Uuid::new_v4()))
        }
        TerminalChoice::Cmd => {
            std::env::temp_dir().join(format!("vibearound-launch-{}.cmd", uuid::Uuid::new_v4()))
        }
        other => bail!("terminal '{}' is not supported on Windows", other.id()),
    };

    let body = match choice {
        TerminalChoice::PowerShell => {
            build_powershell_script(env, command, window_label, workspace)
        }
        TerminalChoice::Cmd => build_cmd_script(env, command, window_label, workspace),
        other => bail!("terminal '{}' is not supported on Windows", other.id()),
    };

    std::fs::write(&script_path, body)
        .with_context(|| format!("write launch script {:?}", script_path))?;
    auth::set_owner_only(&script_path).ok();
    Ok(script_path)
}

#[cfg(target_os = "windows")]
fn build_powershell_script(
    env: &[(String, String)],
    command: &str,
    window_label: &str,
    workspace: &Path,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Write-Host '# VibeAround profile: {}'\n",
        window_label.replace('\'', "''")
    ));
    for (k, v) in env {
        out.push_str(&format!("$env:{} = '{}'\n", k, v.replace('\'', "''")));
    }
    out.push_str(&format!(
        "Set-Location -LiteralPath '{}'\n",
        escape_powershell_single_quoted(&workspace.to_string_lossy())
    ));
    out.push_str(command);
    out.push_str("\n");
    out.push_str("if ($LASTEXITCODE -ne $null -and $LASTEXITCODE -ne 0) {\n");
    out.push_str("  Write-Host \"`nCommand exited with code $LASTEXITCODE\"\n");
    out.push_str("}\n");
    out.push_str("$scriptPath = $MyInvocation.MyCommand.Path\n");
    out.push_str("if ($scriptPath) { Remove-Item -LiteralPath $scriptPath -Force -ErrorAction SilentlyContinue }\n");
    out
}

#[cfg(target_os = "windows")]
fn build_cmd_script(
    env: &[(String, String)],
    command: &str,
    window_label: &str,
    workspace: &Path,
) -> String {
    let mut out = String::new();
    out.push_str("@echo off\r\n");
    out.push_str(&format!("@title VibeAround - {}\r\n", window_label));
    out.push_str(&format!("@echo # VibeAround profile: {}\r\n", window_label));
    for (k, v) in env {
        out.push_str(&format!("set \"{}={}\"\r\n", k, v));
    }
    out.push_str(&format!(
        "cd /d \"{}\"\r\n",
        escape_cmd_quoted(&workspace.to_string_lossy())
    ));
    out.push_str(command);
    out.push_str("\r\n");
    out.push_str("set \"VA_EXIT=%ERRORLEVEL%\"\r\n");
    out.push_str("if not \"%VA_EXIT%\"==\"0\" echo.\r\n");
    out.push_str("if not \"%VA_EXIT%\"==\"0\" echo Command exited with code %VA_EXIT%\r\n");
    out.push_str("del \"%~f0\" >nul 2>nul\r\n");
    out
}

// ---------------------------------------------------------------------------
// Bash script builder
// ---------------------------------------------------------------------------

fn build_bash_script(
    env: &[(String, String)],
    command: &str,
    window_label: &str,
    workspace: &Path,
) -> String {
    let mut out = String::new();
    out.push_str("#!/bin/bash\n");
    // Self-delete first so an unexpected ^C between here and `exec` doesn't
    // leave the credential-bearing script sitting in $TMPDIR.
    out.push_str("rm -- \"$0\"\n");
    out.push_str("set -e\n");
    out.push_str(&format!(
        "echo \"# VibeAround profile: {}\"\n",
        // Window label is shown to the user only — sanitize quotes to keep
        // the echo line well-formed; we do not need full shell escape here
        // because the value isn't a security boundary.
        window_label.replace('"', "'")
    ));

    let mut seen: HashMap<&str, ()> = HashMap::new();
    for (k, v) in env {
        if seen.insert(k.as_str(), ()).is_some() {
            // Catalog should not produce duplicate keys, but `last write
            // wins` is more useful than crashing the launch.
            tracing::warn!("[profiles] duplicate env key '{}' in render output", k);
        }
        let escaped = shell_escape::unix::escape(std::borrow::Cow::Borrowed(v.as_str()));
        out.push_str(&format!("export {}={}\n", k, escaped));
    }

    // `open -a Terminal foo.command` opens the new window with $TMPDIR as
    // CWD; surfacing the user in /private/var/folders/... is jarring. Move
    // to the selected launch workspace before exec so the CLI starts in the
    // project the user intended.
    let workspace_string = workspace.to_string_lossy();
    let cwd = shell_escape::unix::escape(std::borrow::Cow::Borrowed(workspace_string.as_ref()));
    out.push_str(&format!("cd {}\n", cwd));
    out.push_str(&format!("exec {}\n", command));
    out
}

#[cfg(target_os = "windows")]
fn escape_powershell_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(target_os = "windows")]
fn escape_cmd_quoted(value: &str) -> String {
    value.replace('"', "\"\"")
}

#[cfg(target_os = "windows")]
fn quote_cmd_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_bash_script_escapes_injection_payload() {
        let env = vec![(
            "ANTHROPIC_API_KEY".to_string(),
            "hi$(touch /tmp/pwned)".to_string(),
        )];
        let script = build_bash_script(&env, "claude", "Test", Path::new("/tmp/work dir"));
        // The export line must contain the payload as a *literal* — i.e.
        // single-quoted by shell_escape — not as an unquoted command
        // substitution that bash would actually evaluate.
        assert!(
            script.contains("'hi$(touch /tmp/pwned)'"),
            "expected single-quoted payload, got:\n{}",
            script
        );
        assert!(!script.contains("$(touch /tmp/pwned)\n"));
    }

    #[test]
    fn build_bash_script_includes_self_delete_first() {
        let script = build_bash_script(&[], "claude", "x", Path::new("/tmp/work dir"));
        let lines: Vec<&str> = script.lines().collect();
        assert_eq!(lines[0], "#!/bin/bash");
        assert_eq!(lines[1], "rm -- \"$0\"");
    }

    #[test]
    fn build_bash_script_cd_selected_workspace() {
        let script = build_bash_script(&[], "claude", "x", Path::new("/tmp/my project"));
        assert!(script.contains("cd '/tmp/my project'\n"));
    }

    #[test]
    fn rewrites_codex_chat_config_for_proxy() {
        let input = r#"model = "deepseek-chat"
model_provider = "deepseek"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
wire_api = "chat"
requires_openai_auth = true
"#;

        let output = rewrite_codex_config_for_proxy(
            input,
            "deepseek",
            "http://127.0.0.1:12358/va/openai-proxy/deepseek/v1",
        )
        .unwrap();

        assert!(
            output.contains(r#"base_url = "http://127.0.0.1:12358/va/openai-proxy/deepseek/v1""#)
        );
        assert!(output.contains(r#"wire_api = "responses""#));
    }
}
