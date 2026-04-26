//! Terminal-app preferences.
//!
//! v1 supports Terminal.app (always present on macOS) and iTerm2 (detected
//! by `/Applications/iTerm.app` existence). The preference lives in a tiny
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
}

impl TerminalChoice {
    pub const ALL: &'static [TerminalChoice] = &[Self::Terminal, Self::Iterm2];

    pub fn id(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Iterm2 => "iterm2",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal.app",
            Self::Iterm2 => "iTerm2",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "terminal" => Some(Self::Terminal),
            "iterm2" => Some(Self::Iterm2),
            _ => None,
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
    }
}

// ---------------------------------------------------------------------------
// Preference file I/O
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
struct LauncherPrefsFile {
    #[serde(default)]
    terminal: Option<String>,
}

fn prefs_path() -> PathBuf {
    config::data_dir().join("launcher.json")
}

/// Read the user's preferred terminal. Falls back to Terminal.app whenever
/// the prefs file is missing, malformed, or names a terminal we don't
/// recognize anymore (forward-compat: an old prefs file from a future
/// build that knew about more terminals must not crash this version).
pub fn read_preference() -> TerminalChoice {
    let body = match std::fs::read_to_string(prefs_path()) {
        Ok(b) => b,
        Err(_) => return TerminalChoice::Terminal,
    };
    let prefs: LauncherPrefsFile = match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[launcher] launcher.json parse error: {} — using default", e);
            return TerminalChoice::Terminal;
        }
    };
    prefs
        .terminal
        .as_deref()
        .and_then(TerminalChoice::from_id)
        .unwrap_or(TerminalChoice::Terminal)
}

pub fn write_preference(choice: TerminalChoice) -> anyhow::Result<()> {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {:?}", parent))?;
    }
    let prefs = LauncherPrefsFile {
        terminal: Some(choice.id().to_string()),
    };
    let body = serde_json::to_string_pretty(&prefs).context("serialize launcher prefs")?;
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
}
