//! Profiles — user-managed third-party API credentials + one-click launch
//! into a system Terminal.app window with the right env vars injected.
//!
//! The schema/catalog/rendering engine lives in `common::profiles` so the
//! headless core can launch IM agents with the same profile behavior.

mod launcher;
mod terminal;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use common::profiles::{catalog, normalize_legacy_profile, runtime, schema};
use common::{config, resources};
use serde::Serialize;
use tauri::Emitter;

pub use common::profiles::{AuthMode, ProfileDef};

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
    ordered_profiles()
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
            let api_type_warnings_for_targets = api_type_warnings.clone();
            ProfileSummary {
                id: p.id,
                label: p.label,
                provider: p.provider,
                provider_label: label,
                provider_icon: icon,
                auth_mode: p.auth_mode,
                launch_targets: runtime::launch_targets_for_api_types(&p.api_types)
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
pub fn profiles_upsert(app: tauri::AppHandle, profile: ProfileDef) -> Result<(), String> {
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
    schema::save(&profile).map_err(|e| e.to_string())?;
    ensure_profile_order_contains(&profile.id)?;
    emit_launch_config_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn profiles_delete(app: tauri::AppHandle, id: String) -> Result<(), String> {
    schema::delete(&id).map_err(|e| e.to_string())?;
    clear_default_profile_references(&id)?;
    emit_launch_config_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn profiles_reorder(app: tauri::AppHandle, profile_ids: Vec<String>) -> Result<(), String> {
    let profiles: Vec<_> = schema::list()
        .into_iter()
        .map(normalize_legacy_profile)
        .collect();
    let existing_ids: HashSet<_> = profiles.iter().map(|profile| profile.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut ordered_ids = Vec::new();

    for id in profile_ids {
        let id = id.trim();
        if existing_ids.contains(id) && seen.insert(id.to_string()) {
            ordered_ids.push(id.to_string());
        }
    }

    for profile in profiles {
        if seen.insert(profile.id.clone()) {
            ordered_ids.push(profile.id);
        }
    }

    write_profile_order(&ordered_ids)?;
    emit_launch_config_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn profiles_launch(id: String, launch_target: String) -> Result<(), String> {
    let profile = schema::load(&id)
        .map(normalize_legacy_profile)
        .ok_or_else(|| format!("profile '{id}' not found"))?;
    if !runtime::launch_targets_for_api_types(&profile.api_types)
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
    /// Resolved cwd used for profile/direct launches.
    pub workspace: String,
    /// Suggested cwd choices surfaced in the Launch header.
    pub workspace_options: Vec<WorkspaceOption>,
    /// Canonical agent id used by Quick Launch and IM defaults.
    pub default_agent: String,
    /// Per-agent profile defaults from settings.json.
    pub default_profiles: std::collections::BTreeMap<String, String>,
    /// Global policy for wrapping OpenAI-compatible profile launches through
    /// VibeAround's local compatibility proxy.
    pub compatibility_proxy: terminal::CompatibilityProxyMode,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOption {
    pub path: String,
    pub label: String,
    pub detail: String,
    pub kind: String,
    pub is_default: bool,
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
    let workspace_options = launcher_workspace_options();
    let workspace = terminal::resolve_workspace_preference()
        .unwrap_or_else(|_| terminal::launch_home_dir().unwrap_or_else(|_| config::data_dir()))
        .to_string_lossy()
        .to_string();
    let cfg = config::ensure_loaded();
    LauncherPreferences {
        terminal: terminal::read_preference().id().to_string(),
        options,
        workspace,
        workspace_options,
        default_agent: canonical_agent_id(&cfg.default_agent),
        default_profiles: cfg.default_profiles.clone(),
        compatibility_proxy: terminal::read_compatibility_proxy_preference(),
    }
}

#[tauri::command]
pub fn profiles_launch_default() -> Result<(), String> {
    let cfg = config::ensure_loaded();
    let agent_id = canonical_agent_id(&cfg.default_agent);
    if let Some(profile_id) = cfg.default_profile_for(&agent_id) {
        if let Some(profile) = schema::load(&profile_id).map(normalize_legacy_profile) {
            if runtime::launch_targets_for_api_types(&profile.api_types)
                .iter()
                .any(|(target, _, _)| *target == agent_id)
            {
                return launcher::launch(&profile, &agent_id).map_err(|e| e.to_string());
            }
        }
    }
    launcher::launch_direct(&agent_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn launcher_set_default(
    app: tauri::AppHandle,
    agent_id: String,
    profile_id: Option<String>,
) -> Result<(), String> {
    let agent_id = resources::agent_by_alias(&agent_id)
        .map(|def| def.id.clone())
        .ok_or_else(|| format!("unknown agent: '{agent_id}'"))?;
    let profile_id = profile_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());

    if let Some(profile_id) = &profile_id {
        let profile = schema::load(profile_id)
            .map(normalize_legacy_profile)
            .ok_or_else(|| format!("profile '{profile_id}' not found"))?;
        if !runtime::launch_targets_for_api_types(&profile.api_types)
            .iter()
            .any(|(target, _, _)| *target == agent_id)
        {
            return Err(format!("profile '{profile_id}' cannot launch '{agent_id}'"));
        }
    }

    config::update_settings_json(|root| {
        if let Some(obj) = root.as_object_mut() {
            obj.insert("default_agent".into(), serde_json::json!(agent_id.clone()));
            let default_profiles = obj
                .entry("default_profiles")
                .or_insert_with(|| serde_json::json!({}));
            if !default_profiles.is_object() {
                *default_profiles = serde_json::json!({});
            }
            if let Some(map) = default_profiles.as_object_mut() {
                match &profile_id {
                    Some(profile_id) => {
                        map.insert(agent_id.clone(), serde_json::json!(profile_id));
                    }
                    None => {
                        map.remove(&agent_id);
                    }
                }
            }
        }
    })
    .map_err(|e| e.to_string())?;
    emit_launch_config_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn launcher_set_terminal(terminal_id: String) -> Result<(), String> {
    let choice = terminal::TerminalChoice::from_id(&terminal_id)
        .ok_or_else(|| format!("unknown terminal: '{}'", terminal_id))?;
    terminal::write_preference(choice).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn launcher_set_workspace(workspace_path: String) -> Result<(), String> {
    terminal::write_workspace_preference(PathBuf::from(workspace_path)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn launcher_set_compatibility_proxy(app: tauri::AppHandle, mode: String) -> Result<(), String> {
    let mode = terminal::CompatibilityProxyMode::from_id(&mode)
        .ok_or_else(|| format!("unknown compatibility proxy mode: '{mode}'"))?;
    terminal::write_compatibility_proxy_preference(mode).map_err(|e| e.to_string())?;
    emit_launch_config_changed(&app);
    Ok(())
}

fn launcher_workspace_options() -> Vec<WorkspaceOption> {
    let cfg = config::ensure_loaded();
    let builtin = config::builtin_workspaces_dir();
    let home = terminal::launch_home_dir().unwrap_or_else(|_| config::data_dir());
    let selected = terminal::resolve_workspace_preference().ok();
    let default_workspace = cfg
        .default_workspace
        .clone()
        .unwrap_or_else(|| builtin.clone());

    let mut out = Vec::new();
    push_workspace_option(&mut out, &home, "Home", "home", false);
    for workspace in cfg.all_workspaces() {
        let is_default = paths_equal(&workspace, &default_workspace);
        let kind = if paths_equal(&workspace, &builtin) {
            "built-in"
        } else {
            "workspace"
        };
        let label = if is_default {
            "Default workspace".to_string()
        } else {
            path_label(&workspace)
        };
        push_workspace_option(&mut out, &workspace, &label, kind, is_default);
    }
    if let Some(path) = selected {
        if !out
            .iter()
            .any(|option| paths_equal(Path::new(&option.path), &path))
        {
            let label = path_label(&path);
            push_workspace_option(&mut out, &path, &label, "selected", false);
        }
    }
    out
}

fn canonical_agent_id(agent_id: &str) -> String {
    resources::agent_by_alias(agent_id)
        .map(|def| def.id.clone())
        .unwrap_or_else(|| agent_id.to_string())
}

pub(crate) fn ordered_profiles() -> Vec<ProfileDef> {
    let mut remaining: Vec<_> = schema::list()
        .into_iter()
        .map(normalize_legacy_profile)
        .collect();
    let mut out = Vec::new();

    for id in read_profile_order() {
        if let Some(index) = remaining.iter().position(|profile| profile.id == id) {
            out.push(remaining.remove(index));
        }
    }

    out.extend(remaining);
    out
}

fn clear_default_profile_references(profile_id: &str) -> Result<(), String> {
    config::update_settings_json(|root| {
        if let Some(obj) = root.as_object_mut() {
            let mut remove_default_profiles = false;
            if let Some(map) = obj
                .get_mut("default_profiles")
                .and_then(|value| value.as_object_mut())
            {
                map.retain(|_, value| value.as_str() != Some(profile_id));
                remove_default_profiles = map.is_empty();
            }
            if remove_default_profiles {
                obj.remove("default_profiles");
            }
            if let Some(order) = obj
                .get_mut("profile_order")
                .and_then(|value| value.as_array_mut())
            {
                order.retain(|value| value.as_str() != Some(profile_id));
                if order.is_empty() {
                    obj.remove("profile_order");
                }
            }
        }
    })
    .map_err(|e| e.to_string())
}

fn read_profile_order() -> Vec<String> {
    let path = config::data_dir().join("settings.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
        .and_then(|root| {
            root.get("profile_order")
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(ToOwned::to_owned)
                        .collect()
                })
        })
        .unwrap_or_default()
}

fn write_profile_order(profile_ids: &[String]) -> Result<(), String> {
    config::update_settings_json(|root| {
        if let Some(obj) = root.as_object_mut() {
            obj.insert(
                "profile_order".to_string(),
                serde_json::Value::Array(
                    profile_ids
                        .iter()
                        .map(|id| serde_json::Value::String(id.clone()))
                        .collect(),
                ),
            );
        }
    })
    .map_err(|e| e.to_string())
}

fn ensure_profile_order_contains(profile_id: &str) -> Result<(), String> {
    let mut order = read_profile_order();
    if !order.iter().any(|id| id == profile_id) {
        order.push(profile_id.to_string());
        write_profile_order(&order)?;
    }
    Ok(())
}

fn emit_launch_config_changed(app: &tauri::AppHandle) {
    let _ = app.emit(crate::tray::LAUNCH_CONFIG_CHANGED_EVENT, ());
}

fn push_workspace_option(
    out: &mut Vec<WorkspaceOption>,
    path: &Path,
    label: &str,
    kind: &str,
    is_default: bool,
) {
    if out
        .iter()
        .any(|option| paths_equal(Path::new(&option.path), path))
    {
        return;
    }
    out.push(WorkspaceOption {
        path: path.to_string_lossy().to_string(),
        label: label.to_string(),
        detail: path.to_string_lossy().to_string(),
        kind: kind.to_string(),
        is_default,
    });
}

fn path_label(path: &Path) -> String {
    if let Some(name) = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
    {
        name.to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
        || std::fs::canonicalize(left)
            .ok()
            .zip(std::fs::canonicalize(right).ok())
            .map(|(left, right)| left == right)
            .unwrap_or(false)
}
