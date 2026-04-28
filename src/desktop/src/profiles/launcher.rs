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
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};

use common::{auth, config, resources};

use super::catalog;
use super::render::{render, ConfigEnvTarget};
use super::schema::ProfileDef;
use super::terminal::{self, TerminalChoice};

// ---------------------------------------------------------------------------
// Public entry
// ---------------------------------------------------------------------------

pub fn launch(profile: &ProfileDef, launch_target: &str) -> anyhow::Result<()> {
    let provider = catalog::get(&profile.provider)
        .ok_or_else(|| anyhow!("unknown provider '{}'", profile.provider))?;
    let api_type = api_type_for_launch_target(profile, provider, launch_target)?;
    let rendered = render(profile, api_type, launch_target, provider)?;
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
    rendered: super::render::RenderedProfile,
) -> anyhow::Result<()> {
    let mut env: Vec<(String, String)> = rendered.env.clone();
    if !rendered.settings_files.is_empty() {
        let dir = profile_state_dir(&profile.id);
        for sf in &rendered.settings_files {
            materialize_settings_file(&dir, &sf.rel_path, &sf.contents)?;
        }
        if let Some(target) = rendered.config_env {
            match target {
                ConfigEnvTarget::Directory(env_name) => {
                    env.push((env_name.to_string(), dir.to_string_lossy().into_owned()));
                }
                ConfigEnvTarget::File {
                    env: env_name,
                    rel_path,
                } => {
                    env.push((
                        env_name.to_string(),
                        dir.join(rel_path).to_string_lossy().into_owned(),
                    ));
                }
            }
        }
    }

    let agent_id = agent_id_for(launch_target)?;
    let agent = resources::agent_by_id(agent_id)
        .ok_or_else(|| anyhow!("agent '{}' not found in agents.json", agent_id))?;
    let pty_command = command_with_args(&agent.pty.command, &rendered.command_args);
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

fn agent_id_for(launch_target: &str) -> anyhow::Result<&'static str> {
    match launch_target {
        "claude" => Ok("claude"),
        "codex" => Ok("codex"),
        "gemini" => Ok("gemini"),
        "opencode" => Ok("opencode"),
        other => bail!("unsupported launch target: '{}'", other),
    }
}

fn api_type_for_launch_target<'a>(
    profile: &'a ProfileDef,
    provider: &'a catalog::ProviderCatalog,
    launch_target: &str,
) -> anyhow::Result<&'a str> {
    let candidates: &[&str] = match launch_target {
        "claude" => &["anthropic"],
        // Prefer Responses once a provider declares it; fall back to the
        // OpenAI-compatible chat endpoint that today's catalog uses.
        "codex" => &["openai-responses", "openai-chat"],
        "gemini" => &["gemini"],
        // OpenCode is a CLI target, not a provider protocol. Prefer
        // Responses for GPT-5.x/tool-heavy models, then fall back to
        // Chat Completions for providers that only expose OpenAI-compatible
        // chat.
        "opencode" => &["openai-responses", "openai-chat"],
        other => bail!("unsupported launch target: '{}'", other),
    };

    for candidate in candidates {
        if profile.api_types.iter().any(|t| t == candidate)
            && provider.endpoints.iter().any(|e| e.api_type == *candidate)
        {
            return Ok(candidate);
        }
    }

    bail!(
        "profile '{}' cannot launch '{}' with provider '{}'",
        profile.id,
        launch_target,
        profile.provider
    )
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

fn profile_state_dir(id: &str) -> PathBuf {
    config::data_dir().join("profile-state").join(id)
}

fn materialize_settings_file(dir: &Path, rel_path: &str, contents: &str) -> anyhow::Result<()> {
    let target = dir.join(rel_path);

    // Defense in depth: even though render::validate_rel_path already
    // rejects `..` segments, double-check after canonicalization that the
    // resolved target lives inside the per-profile state dir. This catches
    // catalog templates whose rel_path is a non-traversal symlink that
    // points outside (the parent dir doesn't exist yet on first launch, so
    // we canonicalize the parent we're about to create instead).
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {:?}", parent))?;
        let canonical_parent =
            std::fs::canonicalize(parent).with_context(|| format!("canonicalize {:?}", parent))?;
        let canonical_root =
            std::fs::canonicalize(dir).with_context(|| format!("canonicalize {:?}", dir))?;
        if !canonical_parent.starts_with(&canonical_root) {
            bail!(
                "rendered settings_file escapes profile-state dir: {:?}",
                target
            );
        }
    }

    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, contents).with_context(|| format!("write {:?}", tmp))?;
    auth::set_owner_only(&tmp).ok();
    std::fs::rename(&tmp, &target).with_context(|| format!("rename to {:?}", target))?;
    Ok(())
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

    let mut starter = std::process::Command::new("cmd.exe");
    starter
        .arg("/C")
        .arg("start")
        .arg(&title)
        .current_dir(workspace);

    match choice {
        TerminalChoice::PowerShell => {
            starter
                .arg("powershell.exe")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-NoExit")
                .arg("-File")
                .arg(&script_path);
        }
        TerminalChoice::Cmd => {
            starter.arg("cmd.exe").arg("/K").arg(&script_path);
        }
        other => bail!("terminal '{}' is not supported on Windows", other.id()),
    }

    starter
        .spawn()
        .with_context(|| format!("open {}", choice.label()))?;

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
}
