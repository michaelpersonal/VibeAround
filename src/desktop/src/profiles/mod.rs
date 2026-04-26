//! Profiles — user-managed third-party API credentials + one-click launch
//! into a system Terminal.app window with the right env vars injected.
//!
//! See `schema.rs` for the on-disk layout, `catalog.rs` for the built-in
//! provider metadata, `render.rs` for the env / settings-file engine, and
//! `launcher.rs` for the macOS Terminal spawn path.

mod catalog;
mod launcher;
mod render;
mod schema;
mod terminal;

use serde::Serialize;

pub use schema::{AuthMode, ProfileDef};

// ---------------------------------------------------------------------------
// View types — sanitized for the frontend.
// ---------------------------------------------------------------------------

/// List item — does NOT include credentials. Used to render the Launch tab
/// without ever shipping API keys to the webview after the initial save.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub id: String,
    pub label: String,
    pub provider: String,
    /// Provider's display label, resolved from the catalog. Falls back to
    /// the raw provider id when the catalog entry is missing — this can
    /// happen if a user keeps a profile after we ship a catalog removal.
    pub provider_label: String,
    pub provider_icon: Option<String>,
    pub auth_mode: AuthMode,
    pub api_types: Vec<String>,
    /// `api_type → caveat string` (subset; only the api_types that have a
    /// non-empty `compatibility_warning` in the catalog appear here). Lets
    /// the UI render a ⚠ tooltip on the affected launch button without
    /// needing the full catalog client-side.
    pub api_type_warnings: std::collections::BTreeMap<String, String>,
}

/// Catalog entry sent to the UI. Nested `EndpointDef` / `AuthModeDef` /
/// `FieldDef` types use snake_case keys (no rename annotation) so the
/// frontend's mustache-lite knowledge of `{{api_key}}` / `{{base_url}}`
/// stays consistent end-to-end.
#[derive(Debug, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub homepage: Option<String>,
    pub endpoints: Vec<catalog::EndpointDef>,
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn profiles_list() -> Vec<ProfileSummary> {
    schema::list()
        .into_iter()
        .map(|p| {
            let provider = catalog::get(&p.provider);
            let (label, icon) = match provider {
                Some(c) => (c.label.clone(), c.icon.clone()),
                None => (p.provider.clone(), None),
            };
            let mut api_type_warnings: std::collections::BTreeMap<String, String> =
                std::collections::BTreeMap::new();
            if let Some(c) = provider {
                for api_type in &p.api_types {
                    if let Some(ep) = c.endpoints.iter().find(|e| &e.api_type == api_type) {
                        if let Some(w) = &ep.compatibility_warning {
                            api_type_warnings.insert(api_type.clone(), w.clone());
                        }
                    }
                }
            }
            ProfileSummary {
                id: p.id,
                label: p.label,
                provider: p.provider,
                provider_label: label,
                provider_icon: icon,
                auth_mode: p.auth_mode,
                api_types: p.api_types,
                api_type_warnings,
            }
        })
        .collect()
}

#[tauri::command]
pub fn profiles_get(id: String) -> Result<ProfileDef, String> {
    schema::load(&id).ok_or_else(|| format!("profile '{id}' not found"))
}

#[tauri::command]
pub fn profiles_upsert(profile: ProfileDef) -> Result<(), String> {
    schema::validate(&profile).map_err(|e| e.to_string())?;
    let provider = catalog::get(&profile.provider)
        .ok_or_else(|| format!("unknown provider '{}'", profile.provider))?;
    for api_type in &profile.api_types {
        if !provider.endpoints.iter().any(|e| &e.api_type == api_type) {
            return Err(format!(
                "provider '{}' does not support api_type '{}'",
                profile.provider, api_type
            ));
        }
    }
    schema::save(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn profiles_delete(id: String) -> Result<(), String> {
    schema::delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn profiles_launch(id: String, api_type: String) -> Result<(), String> {
    let profile = schema::load(&id).ok_or_else(|| format!("profile '{id}' not found"))?;
    if !profile.api_types.contains(&api_type) {
        return Err(format!(
            "profile '{id}' does not declare api_type '{api_type}'"
        ));
    }
    launcher::launch(&profile, &api_type).map_err(|e| e.to_string())
}

/// Launch a CLI directly with no env injection — uses whatever global
/// OAuth / login session the user already has. `agent_id` is the
/// agents.json id (e.g. "claude", "codex", "gemini", "cursor", "kiro",
/// "qwen-code", "opencode").
#[tauri::command]
pub fn profiles_launch_direct(agent_id: String) -> Result<(), String> {
    launcher::launch_direct(&agent_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn profiles_catalog() -> Vec<CatalogEntry> {
    catalog::all()
        .iter()
        .map(|c| CatalogEntry {
            id: c.id.clone(),
            label: c.label.clone(),
            icon: c.icon.clone(),
            homepage: c.homepage.clone(),
            endpoints: c.endpoints.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Terminal preference commands
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOption {
    pub id: String,
    pub label: String,
    pub installed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherPreferences {
    /// `id` of the currently-preferred terminal.
    pub terminal: String,
    /// Every supported terminal, with an `installed` flag the UI uses to
    /// gray out unavailable choices instead of just hiding them — keeps
    /// the dropdown stable and discoverable as users install more apps.
    pub options: Vec<TerminalOption>,
}

#[tauri::command]
pub fn launcher_get_preferences() -> LauncherPreferences {
    let installed_ids: std::collections::HashSet<&'static str> = terminal::detect_installed()
        .iter()
        .map(|c| c.id())
        .collect();
    let options = terminal::TerminalChoice::ALL
        .iter()
        .map(|c| TerminalOption {
            id: c.id().to_string(),
            label: c.label().to_string(),
            installed: installed_ids.contains(c.id()),
        })
        .collect();
    LauncherPreferences {
        terminal: terminal::read_preference().id().to_string(),
        options,
    }
}

#[tauri::command]
pub fn launcher_set_terminal(terminal_id: String) -> Result<(), String> {
    let choice = terminal::TerminalChoice::from_id(&terminal_id)
        .ok_or_else(|| format!("unknown terminal: '{}'", terminal_id))?;
    terminal::write_preference(choice).map_err(|e| e.to_string())
}
