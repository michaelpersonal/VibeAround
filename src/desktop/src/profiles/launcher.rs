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

#[cfg(target_os = "windows")]
use common::auth;
use common::{config, profiles, resources};

use super::terminal::{self, TerminalChoice};
use profiles::ProfileDef;

// ---------------------------------------------------------------------------
// Public entry
// ---------------------------------------------------------------------------

pub fn launch(profile: &ProfileDef, launch_target: &str) -> anyhow::Result<()> {
    let launch_id = uuid::Uuid::new_v4().to_string();
    let mut rendered = profiles::runtime::render_for_launch(profile, launch_target)?;
    apply_compatibility_proxy(profile, launch_target, &launch_id, &mut rendered)?;
    apply_codex_session_hooks(profile, launch_target, &launch_id, &mut rendered)?;
    do_launch(profile, launch_target, &launch_id, rendered)
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
    launch_id: &str,
    rendered: profiles::render::RenderedProfile,
) -> anyhow::Result<()> {
    let command_args = rendered.command_args.clone();
    let mut env = profiles::runtime::materialize_env(&profile.id, rendered)?;
    env.push(("VIBEAROUND_LAUNCH_ID".to_string(), launch_id.to_string()));
    env.push(("VIBEAROUND_PROFILE_ID".to_string(), profile.id.clone()));
    env.push((
        "VIBEAROUND_LAUNCH_TARGET".to_string(),
        launch_target.to_string(),
    ));
    let agent_id = profiles::runtime::agent_id_for(launch_target)?;
    let agent = resources::agent_by_id(agent_id)
        .ok_or_else(|| anyhow!("agent '{}' not found in agents.json", agent_id))?;
    let pty_command = command_with_args(&agent.pty.command, &command_args);
    let workspace = terminal::resolve_workspace_preference()?;

    spawn_terminal(&env, &pty_command, &profile.label, &workspace)?;
    Ok(())
}

fn apply_codex_session_hooks(
    profile: &ProfileDef,
    launch_target: &str,
    launch_id: &str,
    rendered: &mut profiles::render::RenderedProfile,
) -> anyhow::Result<()> {
    if launch_target != "codex" {
        return Ok(());
    }

    let hook_helper = resolve_hook_helper_path()?;
    push_codex_config_arg(&mut rendered.command_args, "features.codex_hooks", "true");

    let command_for = |event: &str| {
        build_hook_command(
            &hook_helper,
            event,
            launch_id,
            &profile.id,
            launch_target,
            &format!("http://127.0.0.1:{}", config::DEFAULT_PORT),
        )
    };
    push_codex_config_arg(
        &mut rendered.command_args,
        "hooks.SessionStart",
        &codex_hook_config_value(Some("startup|resume|clear"), &command_for("SessionStart")),
    );
    push_codex_config_arg(
        &mut rendered.command_args,
        "hooks.UserPromptSubmit",
        &codex_hook_config_value(None, &command_for("UserPromptSubmit")),
    );
    push_codex_config_arg(
        &mut rendered.command_args,
        "hooks.Stop",
        &codex_hook_config_value(None, &command_for("Stop")),
    );
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
    launch_id: &str,
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
        "http://127.0.0.1:{}/va/openai-proxy/{}/{}/v1",
        config::DEFAULT_PORT,
        profile.id,
        launch_id
    );

    let provider_key = format!("model_providers.{}", profile.provider);
    push_codex_config_arg(
        &mut rendered.command_args,
        &format!("{provider_key}.base_url"),
        &toml_basic_string(&proxy_base_url),
    );
    push_codex_config_arg(
        &mut rendered.command_args,
        &format!("{provider_key}.wire_api"),
        &toml_basic_string("responses"),
    );

    Ok(())
}

fn push_codex_config_arg(args: &mut Vec<String>, key: &str, value: &str) {
    args.push("-c".to_string());
    args.push(format!("{key}={value}"));
}

fn codex_hook_config_value(matcher: Option<&str>, command: &str) -> String {
    let mut fields = Vec::new();
    if let Some(matcher) = matcher {
        fields.push(format!("matcher = {}", toml_basic_string(matcher)));
    }
    fields.push(format!(
        "hooks = [{{ type = \"command\", command = {}, timeout = 5 }}]",
        toml_basic_string(command)
    ));
    format!("[{{ {} }}]", fields.join(", "))
}

fn build_hook_command(
    hook_helper: &Path,
    event: &str,
    launch_id: &str,
    profile_id: &str,
    launch_target: &str,
    server_url: &str,
) -> String {
    [
        quote_hook_arg(&hook_helper.to_string_lossy()),
        "--agent".to_string(),
        quote_hook_arg("codex"),
        "--event".to_string(),
        quote_hook_arg(event),
        "--launch-id".to_string(),
        quote_hook_arg(launch_id),
        "--profile-id".to_string(),
        quote_hook_arg(profile_id),
        "--launch-target".to_string(),
        quote_hook_arg(launch_target),
        "--server".to_string(),
        quote_hook_arg(server_url),
    ]
    .join(" ")
}

fn resolve_hook_helper_path() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe().context("resolve current executable")?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| anyhow!("current executable has no parent: {:?}", exe))?;
    let sidecar_dir = if exe_dir.ends_with("deps") {
        exe_dir.parent().unwrap_or(exe_dir)
    } else {
        exe_dir
    };
    let candidate = sidecar_dir.join(hook_helper_name());
    if candidate.exists() {
        return Ok(candidate);
    }

    #[cfg(debug_assertions)]
    {
        let dev_candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/debug")
            .join(hook_helper_name());
        if dev_candidate.exists() {
            return Ok(dev_candidate);
        }
    }

    bail!(
        "VibeAround hook helper not found next to executable: {:?}",
        candidate
    )
}

fn hook_helper_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "vibearound-hook.exe"
    } else {
        "vibearound-hook"
    }
}

#[cfg(not(target_os = "windows"))]
fn quote_hook_arg(value: &str) -> String {
    shell_escape::unix::escape(std::borrow::Cow::Borrowed(value)).into_owned()
}

#[cfg(target_os = "windows")]
fn quote_hook_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
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
    append_powershell_color_env(&mut out);
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
    append_cmd_color_env(&mut out);
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
    append_bash_color_env(&mut out);

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

fn append_bash_color_env(out: &mut String) {
    out.push_str("unset NO_COLOR\n");
    out.push_str(
        "if [ -z \"${TERM:-}\" ] || [ \"$TERM\" = \"dumb\" ]; then export TERM=xterm-256color; fi\n",
    );
    out.push_str("export COLORTERM=${COLORTERM:-truecolor}\n");
    out.push_str("export CLICOLOR=${CLICOLOR:-1}\n");
}

#[cfg(target_os = "windows")]
fn escape_powershell_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(target_os = "windows")]
fn append_powershell_color_env(out: &mut String) {
    out.push_str("Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue\n");
    out.push_str("if (-not $env:TERM -or $env:TERM -eq 'dumb') { $env:TERM = 'xterm-256color' }\n");
    out.push_str("if (-not $env:COLORTERM) { $env:COLORTERM = 'truecolor' }\n");
    out.push_str("if (-not $env:CLICOLOR) { $env:CLICOLOR = '1' }\n");
}

#[cfg(target_os = "windows")]
fn escape_cmd_quoted(value: &str) -> String {
    value.replace('"', "\"\"")
}

#[cfg(target_os = "windows")]
fn append_cmd_color_env(out: &mut String) {
    out.push_str("set \"NO_COLOR=\"\r\n");
    out.push_str("if not defined TERM set \"TERM=xterm-256color\"\r\n");
    out.push_str("if /I \"%TERM%\"==\"dumb\" set \"TERM=xterm-256color\"\r\n");
    out.push_str("if not defined COLORTERM set \"COLORTERM=truecolor\"\r\n");
    out.push_str("if not defined CLICOLOR set \"CLICOLOR=1\"\r\n");
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
    fn build_bash_script_restores_color_capable_terminal_env() {
        let env = vec![("NO_COLOR".to_string(), "1".to_string())];
        let script = build_bash_script(&env, "codex", "x", Path::new("/tmp/work dir"));

        assert!(script.contains("export NO_COLOR=1\n"));
        assert!(script.contains("unset NO_COLOR\n"));
        assert!(script.contains("export TERM=xterm-256color"));
        assert!(script.contains("export COLORTERM=${COLORTERM:-truecolor}\n"));
        assert!(script.contains("export CLICOLOR=${CLICOLOR:-1}\n"));
        assert!(script.find("export NO_COLOR=1").unwrap() < script.find("unset NO_COLOR").unwrap());
    }

    #[test]
    fn appends_codex_config_args_for_proxy() {
        let mut args = Vec::new();
        push_codex_config_arg(
            &mut args,
            "model_providers.deepseek.base_url",
            &toml_basic_string("http://127.0.0.1:12358/va/openai-proxy/deepseek/launch-123/v1"),
        );
        push_codex_config_arg(
            &mut args,
            "model_providers.deepseek.wire_api",
            &toml_basic_string("responses"),
        );

        assert_eq!(
            args,
            vec![
                "-c".to_string(),
                "model_providers.deepseek.base_url=\"http://127.0.0.1:12358/va/openai-proxy/deepseek/launch-123/v1\"".to_string(),
                "-c".to_string(),
                "model_providers.deepseek.wire_api=\"responses\"".to_string(),
            ]
        );
    }

    #[test]
    fn builds_codex_hook_config_arg_value() {
        let command = build_hook_command(
            Path::new("/Applications/VibeAround.app/Contents/MacOS/vibearound-hook"),
            "SessionStart",
            "launch-123",
            "profile-456",
            "codex",
            "http://127.0.0.1:12358",
        );
        let value = codex_hook_config_value(Some("startup|resume|clear"), &command);

        assert!(value.starts_with("[{ matcher = \"startup|resume|clear\""));
        assert!(value.contains("hooks = [{ type = \"command\""));
        assert!(value.contains("--launch-id launch-123"));
        assert!(value.contains("--profile-id profile-456"));
        assert!(value.ends_with(" }]"));
    }
}
