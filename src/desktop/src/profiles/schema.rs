//! Profile JSON schema + on-disk CRUD.
//!
//! Each profile is a single flat file at `~/.vibearound/profiles/<id>.json`
//! holding the user's third-party API credentials plus the catalog provider
//! id that describes how to render env / settings files for that endpoint.
//!
//! Profile id == filename stem; the schema enforces that they match so
//! a `cp foo.json bar.json` rename doesn't leave a stale internal id.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};

use common::{auth, config};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    ApiKey,
    OauthViaCli,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ApiTypeOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileDef {
    pub id: String,
    pub label: String,
    /// Catalog provider id (e.g. `"moonshot"`). Reserved value `"custom"` is
    /// not yet supported in v1; UI gates this.
    pub provider: String,
    pub auth_mode: AuthMode,
    /// Which CLI launch targets this credential is good for. Internally these
    /// are still keyed by the API/config shape each target needs.
    pub api_types: Vec<String>,
    /// Free-form credentials — `api_key` is the only field used by v1
    /// catalog entries, but we keep the bag generic so future plugins can
    /// declare custom field names without a schema migration.
    #[serde(default)]
    pub credentials: BTreeMap<String, String>,
    /// Optional per-api-type overrides for `base_url` / `model`. Empty ==
    /// inherit catalog defaults.
    #[serde(default)]
    pub overrides: BTreeMap<String, ApiTypeOverrides>,
}

// ---------------------------------------------------------------------------
// Filesystem layout
// ---------------------------------------------------------------------------

pub fn profiles_dir() -> PathBuf {
    config::data_dir().join("profiles")
}

fn profile_path(id: &str) -> PathBuf {
    profiles_dir().join(format!("{id}.json"))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Profile ids form filenames + are exposed to shells; constrain them to a
/// safe alphabet so a malicious id can't escape the profiles directory or
/// confuse downstream consumers.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

pub fn validate(profile: &ProfileDef) -> anyhow::Result<()> {
    if !is_valid_id(&profile.id) {
        bail!(
            "invalid profile id '{}': must match ^[a-z0-9_-]{{1,64}}$",
            profile.id
        );
    }
    if profile.label.trim().is_empty() {
        bail!("profile label must not be empty");
    }
    if profile.api_types.is_empty() {
        bail!("profile must declare at least one api kind");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

pub fn list() -> Vec<ProfileDef> {
    let dir = profiles_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match load_path(&path) {
            Ok(profile) => {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                if profile.id != stem {
                    tracing::warn!(
                        "[profiles] skipping {:?}: id '{}' != filename stem '{}'",
                        path,
                        profile.id,
                        stem
                    );
                    continue;
                }
                out.push(profile);
            }
            Err(e) => {
                tracing::warn!("[profiles] skipping {:?}: {}", path, e);
            }
        }
    }
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    out
}

pub fn load(id: &str) -> Option<ProfileDef> {
    if !is_valid_id(id) {
        return None;
    }
    load_path(&profile_path(id)).ok()
}

fn load_path(path: &Path) -> anyhow::Result<ProfileDef> {
    let body = std::fs::read_to_string(path).with_context(|| format!("read {:?}", path))?;
    let profile: ProfileDef =
        serde_json::from_str(&body).with_context(|| format!("parse {:?}", path))?;
    Ok(profile)
}

pub fn save(profile: &ProfileDef) -> anyhow::Result<()> {
    validate(profile)?;
    let dir = profiles_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;
    // Lock down the profiles dir on Unix so other local users can't
    // enumerate or read API keys.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    let target = profile_path(&profile.id);
    let tmp = dir.join(format!(".{}.tmp.{}.json", profile.id, std::process::id()));
    let body = serde_json::to_string_pretty(profile).context("serialize profile")?;
    std::fs::write(&tmp, body).with_context(|| format!("write {:?}", tmp))?;
    auth::set_owner_only(&tmp).ok();
    std::fs::rename(&tmp, &target).with_context(|| format!("rename to {:?}", target))?;
    Ok(())
}

pub fn delete(id: &str) -> anyhow::Result<()> {
    if !is_valid_id(id) {
        return Err(anyhow!("invalid profile id '{}'", id));
    }
    let path = profile_path(id);
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(&path).with_context(|| format!("remove {:?}", path))?;
    // Best-effort: also drop the per-profile state dir (rendered settings
    // files, future agent session caches). If the user re-creates a profile
    // with the same id later, we want a clean slate.
    let state_dir = config::data_dir().join("profile-state").join(id);
    let _ = std::fs::remove_dir_all(&state_dir);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_alphabet_accepts_lowercase_alnum_dash_underscore() {
        assert!(is_valid_id("kimi"));
        assert!(is_valid_id("kimi-personal"));
        assert!(is_valid_id("kimi_personal"));
        assert!(is_valid_id("a1"));
    }

    #[test]
    fn id_alphabet_rejects_unsafe_chars() {
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("Kimi"));
        assert!(!is_valid_id("kimi/etc"));
        assert!(!is_valid_id("../etc"));
        assert!(!is_valid_id("kimi.personal"));
        assert!(!is_valid_id(&"a".repeat(65)));
    }
}
