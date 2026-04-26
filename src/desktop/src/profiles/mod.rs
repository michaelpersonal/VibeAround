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
    /// API kinds this provider credential declares, e.g. `anthropic`,
    /// `openai-chat`, `gemini`. Kept as `api_types` on the wire for
    /// profile.json compatibility.
    pub api_types: Vec<String>,
    /// Concrete CLI buttons the Launch tab should render. Derived from the
    /// profile's API kinds plus each CLI target's adapter support.
    pub launch_targets: Vec<LaunchTargetSummary>,
    /// `api_type → caveat string` (subset; only the api_types that have a
    /// non-empty `compatibility_warning` in the catalog appear here). Lets
    /// the UI render a ⚠ tooltip on the affected launch button without
    /// needing the full catalog client-side.
    pub api_type_warnings: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchTargetSummary {
    pub id: String,
    pub label: String,
    pub api_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
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
        .map(normalize_legacy_profile)
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
            let api_type_warnings_for_targets = api_type_warnings.clone();
            ProfileSummary {
                id: p.id,
                label: p.label,
                provider: p.provider,
                provider_label: label,
                provider_icon: icon,
                auth_mode: p.auth_mode,
                launch_targets: launch_targets_for_api_types(&p.api_types)
                    .into_iter()
                    .map(|(id, label, api_type)| LaunchTargetSummary {
                        id: id.to_string(),
                        label: label.to_string(),
                        api_type: api_type.to_string(),
                        warning: api_type_warnings_for_targets.get(api_type).cloned(),
                    })
                    .collect(),
                api_types: p.api_types,
                api_type_warnings,
            }
        })
        .collect()
}

#[tauri::command]
pub fn profiles_get(id: String) -> Result<ProfileDef, String> {
    schema::load(&id)
        .map(normalize_legacy_profile)
        .ok_or_else(|| format!("profile '{id}' not found"))
}

#[tauri::command]
pub fn profiles_upsert(profile: ProfileDef) -> Result<(), String> {
    schema::validate(&profile).map_err(|e| e.to_string())?;
    let provider = catalog::get(&profile.provider)
        .ok_or_else(|| format!("unknown provider '{}'", profile.provider))?;
    for api_type in &profile.api_types {
        if !provider.endpoints.iter().any(|e| &e.api_type == api_type) {
            return Err(format!(
                "provider '{}' does not support api kind '{}'",
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
pub fn profiles_launch(id: String, launch_target: String) -> Result<(), String> {
    let profile = schema::load(&id)
        .map(normalize_legacy_profile)
        .ok_or_else(|| format!("profile '{id}' not found"))?;
    if !launch_targets_for_api_types(&profile.api_types)
        .iter()
        .any(|(target, _, _)| *target == launch_target)
    {
        return Err(format!("profile '{id}' cannot launch '{launch_target}'"));
    }
    launcher::launch(&profile, &launch_target).map_err(|e| e.to_string())
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

fn launch_targets_for_api_types(
    api_types: &[String],
) -> Vec<(&'static str, &'static str, &'static str)> {
    let has = |needle: &str| api_types.iter().any(|t| t == needle);
    let mut out = Vec::new();
    if has("anthropic") {
        out.push(("claude", "Claude Code", "anthropic"));
    }
    if has("openai-responses") {
        out.push(("codex", "Codex", "openai-responses"));
    } else if has("openai-chat") {
        out.push(("codex", "Codex", "openai-chat"));
    }
    if has("gemini") {
        out.push(("gemini", "Gemini CLI", "gemini"));
    }
    if has("openai-responses") {
        out.push(("opencode", "OpenCode", "openai-responses"));
    } else if has("openai-chat") {
        out.push(("opencode", "OpenCode", "openai-chat"));
    }
    out
}

fn normalize_legacy_profile(mut profile: ProfileDef) -> ProfileDef {
    // Azure used to have only one API kind in early catalog iterations.
    // Profiles saved during that window should inherit endpoint/deployment
    // values across both kinds so users can keep editing without retyping.
    // had only one kind should inherit the same endpoint/deployment for both.
    if profile.provider == "azure"
        && profile.api_types.iter().any(|t| t == "openai-responses")
        && !profile.api_types.iter().any(|t| t == "openai-chat")
    {
        profile.api_types.push("openai-chat".to_string());
        if let Some(overrides) = profile.overrides.get("openai-responses").cloned() {
            profile
                .overrides
                .entry("openai-chat".to_string())
                .or_insert(overrides);
        }
    }
    profile
}

#[tauri::command]
pub fn launcher_set_terminal(terminal_id: String) -> Result<(), String> {
    let choice = terminal::TerminalChoice::from_id(&terminal_id)
        .ok_or_else(|| format!("unknown terminal: '{}'", terminal_id))?;
    terminal::write_preference(choice).map_err(|e| e.to_string())
}
