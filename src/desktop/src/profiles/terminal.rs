//! Terminal-app preferences.
//!
//! v1 supports native terminal choices per platform. The preference lives in a tiny
//! dedicated file at `~/.vibearound/launcher.json` so adding it doesn't
//! couple the Launch tab to the daemon's settings.json write path.
//!
//! Adding more terminals (Ghostty, WezTerm, Warp, …) is a matter of:
//!   1. adding a variant to `TerminalChoice`,
//!   2. teaching `detect_installed` how to find it, and
//!   3. adding a `spawn_*` function in `launcher.rs`.
//! No catalog changes; no schema migration.

use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use common::{auth, config};

// ---------------------------------------------------------------------------
// Choice enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalChoice {
    Terminal,
    Iterm2,
    PowerShell,
    Cmd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityProxyMode {
    Auto,
    On,
    Off,
}

impl CompatibilityProxyMode {
    #[cfg(test)]
    pub const ALL: &'static [CompatibilityProxyMode] = &[Self::Auto, Self::On, Self::Off];

    #[cfg(test)]
    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "auto" => Some(Self::Auto),
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

impl TerminalChoice {
    #[cfg(target_os = "macos")]
    pub const ALL: &'static [TerminalChoice] = &[Self::Terminal, Self::Iterm2];
    #[cfg(target_os = "windows")]
    pub const ALL: &'static [TerminalChoice] = &[Self::PowerShell, Self::Cmd];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub const ALL: &'static [TerminalChoice] = &[];

    pub fn id(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Iterm2 => "iterm2",
            Self::PowerShell => "powershell",
            Self::Cmd => "cmd",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal.app",
            Self::Iterm2 => "iTerm2",
            Self::PowerShell => "PowerShell",
            Self::Cmd => "Command Prompt",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "terminal" => Some(Self::Terminal),
            "iterm2" => Some(Self::Iterm2),
            "powershell" => Some(Self::PowerShell),
            "cmd" => Some(Self::Cmd),
            _ => None,
        }
    }

    pub fn default_for_platform() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::PowerShell
        }
        #[cfg(target_os = "macos")]
        {
            Self::Terminal
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Terminal
        }
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Probe the filesystem for which of the supported terminal apps the user
/// actually has installed. Order matches `TerminalChoice::ALL` so the UI
/// can render a stable list.
pub fn detect_installed() -> Vec<TerminalChoice> {
    let mut out = Vec::new();
    for choice in TerminalChoice::ALL {
        if is_installed(*choice) {
            out.push(*choice);
        }
    }
    out
}

fn is_installed(choice: TerminalChoice) -> bool {
    match choice {
        // Terminal.app ships with macOS; assume present.
        TerminalChoice::Terminal => cfg!(target_os = "macos"),
        TerminalChoice::Iterm2 => std::path::Path::new("/Applications/iTerm.app").exists(),
        // Both ship with supported Windows versions.
        TerminalChoice::PowerShell => cfg!(target_os = "windows"),
        TerminalChoice::Cmd => cfg!(target_os = "windows"),
    }
}

// ---------------------------------------------------------------------------
// Preference file I/O
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
struct LauncherPrefsFile {
    #[serde(default)]
    terminal: Option<String>,
    #[serde(default)]
    workspace: Option<PathBuf>,
    #[serde(default)]
    compatibility_proxy: Option<CompatibilityProxyMode>,
}

fn prefs_path() -> PathBuf {
    config::data_dir().join("launcher.json")
}

/// Read the user's preferred terminal. Falls back to Terminal.app whenever
/// the prefs file is missing, malformed, or names a terminal we don't
/// recognize anymore (forward-compat: an old prefs file from a future
/// build that knew about more terminals must not crash this version).
pub fn read_preference() -> TerminalChoice {
    read_prefs_file()
        .terminal
        .as_deref()
        .and_then(TerminalChoice::from_id)
        .filter(|choice| TerminalChoice::ALL.contains(choice))
        .unwrap_or_else(TerminalChoice::default_for_platform)
}

pub fn write_preference(choice: TerminalChoice) -> anyhow::Result<()> {
    let mut prefs = read_prefs_file();
    prefs.terminal = Some(choice.id().to_string());
    write_prefs_file(&prefs)
}

pub fn read_workspace_preference() -> Option<PathBuf> {
    read_prefs_file().workspace
}

pub fn write_workspace_preference(path: PathBuf) -> anyhow::Result<()> {
    let mut prefs = read_prefs_file();
    prefs.workspace = Some(canonical_workspace_path(&path)?);
    write_prefs_file(&prefs)
}

pub fn read_compatibility_proxy_preference() -> CompatibilityProxyMode {
    read_prefs_file()
        .compatibility_proxy
        .unwrap_or(CompatibilityProxyMode::Auto)
}

pub fn write_compatibility_proxy_preference(mode: CompatibilityProxyMode) -> anyhow::Result<()> {
    let mut prefs = read_prefs_file();
    prefs.compatibility_proxy = Some(mode);
    write_prefs_file(&prefs)
}

pub fn resolve_workspace_preference() -> anyhow::Result<PathBuf> {
    match read_workspace_preference() {
        Some(path) => canonical_workspace_path(&path),
        None => launch_home_dir(),
    }
}

pub fn canonical_workspace_path(path: &std::path::Path) -> anyhow::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("workspace does not exist: {}", path.display()))?;
    if !canonical.is_dir() {
        anyhow::bail!("workspace is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

pub fn launch_home_dir() -> anyhow::Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var_os("HOMEDRIVE")?;
                let path = std::env::var_os("HOMEPATH")?;
                Some(PathBuf::from(format!(
                    "{}{}",
                    drive.to_string_lossy(),
                    path.to_string_lossy()
                )))
            })
            .ok_or_else(|| anyhow::anyhow!("could not determine Windows home directory"));
    }

    #[cfg(not(target_os = "windows"))]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"));
    }
}

fn read_prefs_file() -> LauncherPrefsFile {
    let body = match std::fs::read_to_string(prefs_path()) {
        Ok(b) => b,
        Err(_) => return LauncherPrefsFile::default(),
    };
    match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "[launcher] launcher.json parse error: {} — using default",
                e
            );
            LauncherPrefsFile::default()
        }
    }
}

fn write_prefs_file(prefs: &LauncherPrefsFile) -> anyhow::Result<()> {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {:?}", parent))?;
    }
    let body = serde_json::to_string_pretty(prefs).context("serialize launcher prefs")?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).with_context(|| format!("write {:?}", tmp))?;
    auth::set_owner_only(&tmp).ok();
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {:?}", path))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip() {
        for c in TerminalChoice::ALL {
            assert_eq!(TerminalChoice::from_id(c.id()), Some(*c));
        }
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(TerminalChoice::from_id("warp").is_none());
        assert!(TerminalChoice::from_id("").is_none());
    }

    #[test]
    fn compatibility_proxy_mode_ids_roundtrip() {
        for mode in CompatibilityProxyMode::ALL {
            assert_eq!(CompatibilityProxyMode::from_id(mode.id()), Some(*mode));
        }
    }
}
