//! Provider catalog — third-party endpoint metadata baked into the binary.
//!
//! Each provider has a JSON file under `src/resources/profile-catalog/`
//! describing its supported provider API kinds (Anthropic-compatible,
//! OpenAI-compatible, Gemini) plus per-kind auth modes, default base URLs,
//! model lists, and render templates for native CLI adapters.
//!
//! v1 is a static built-in catalog (loaded once via `LazyLock`). The
//! intent is to migrate to a separately-versioned npm package
//! (`@vibearound/provider-catalog`) so that adding / updating providers
//! does not require a desktop release. The Rust types here are the wire
//! schema for that future package — adding fields requires only a serde
//! `#[serde(default)]` to stay forward-compatible.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Embedded JSON sources
// ---------------------------------------------------------------------------
// Note: changing the .json files alone will not trigger a tauri-dev rebuild
// (the file watcher only sees Rust sources). Edit this file or the
// surrounding comment to force a rebuild after touching catalog data.
// catalog-rebuild: 2026-04-27-reasoning-effort-capability

static MOONSHOT_JSON: &str = include_str!("../../../resources/profile-catalog/moonshot.json");
static DEEPSEEK_JSON: &str = include_str!("../../../resources/profile-catalog/deepseek.json");
static OPENROUTER_JSON: &str = include_str!("../../../resources/profile-catalog/openrouter.json");
static MINIMAX_JSON: &str = include_str!("../../../resources/profile-catalog/minimax.json");
static MINIMAX_GLOBAL_JSON: &str =
    include_str!("../../../resources/profile-catalog/minimax-global.json");
static ZAI_JSON: &str = include_str!("../../../resources/profile-catalog/zai.json");
static GEMINI_JSON: &str = include_str!("../../../resources/profile-catalog/gemini.json");
static AZURE_JSON: &str = include_str!("../../../resources/profile-catalog/azure.json");

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderCatalog {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    pub endpoints: Vec<EndpointDef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EndpointDef {
    pub api_type: String,
    pub default_base_url: String,
    #[serde(default)]
    pub models: Vec<ModelDef>,
    #[serde(default)]
    pub capabilities: EndpointCapabilities,
    pub auth_modes: Vec<AuthModeDef>,
    /// Optional caveat shown to users next to the launch button — e.g.
    /// "codex 0.X+ requires Responses API and this provider only serves
    /// chat-completions". `None` for endpoints with no known caveat.
    #[serde(default)]
    pub compatibility_warning: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EndpointCapabilities {
    #[serde(default)]
    pub reasoning_effort: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelDef {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthModeDef {
    pub mode: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub fields: Vec<FieldDef>,
    #[serde(default)]
    pub render: Option<RenderRules>,
    /// Reserved for v2 OAuth flow — not consumed by v1 launcher.
    #[serde(default)]
    pub login_command: Option<Vec<String>>,
    #[serde(default)]
    pub session_file_check: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FieldDef {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub validate: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RenderRules {
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Files to materialize alongside the env exports. Codex specifically
    /// needs *both* `config.toml` (model_provider routing) and `auth.json`
    /// (the API key — codex ignores `OPENAI_API_KEY` env when auth.json
    /// is missing and instead drops into its ChatGPT OAuth welcome screen),
    /// hence the array rather than a single optional file.
    #[serde(default)]
    pub settings_files: Vec<SettingsFileTemplate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SettingsFileTemplate {
    pub rel_path: String,
    pub template: String,
}

// ---------------------------------------------------------------------------
// Static loader
// ---------------------------------------------------------------------------

static CATALOG: LazyLock<Vec<ProviderCatalog>> = LazyLock::new(|| {
    let raw = [
        ("moonshot", MOONSHOT_JSON),
        ("deepseek", DEEPSEEK_JSON),
        ("openrouter", OPENROUTER_JSON),
        ("minimax", MINIMAX_JSON),
        ("minimax-global", MINIMAX_GLOBAL_JSON),
        ("zai", ZAI_JSON),
        ("gemini", GEMINI_JSON),
        ("azure", AZURE_JSON),
    ];
    let mut out = Vec::with_capacity(raw.len());
    for (id, body) in raw {
        match serde_json::from_str::<ProviderCatalog>(body) {
            Ok(c) => out.push(c),
            // Static JSON shipped with the binary — a parse failure here is a
            // build-time bug, not a user error. Crash hard so it shows up in
            // dev rather than silently dropping a provider in release.
            Err(e) => panic!("profile-catalog: failed to parse {}: {}", id, e),
        }
    }
    out
});

pub fn all() -> &'static [ProviderCatalog] {
    &CATALOG
}

pub fn get(id: &str) -> Option<&'static ProviderCatalog> {
    if id == "custom" {
        return Some(custom());
    }
    CATALOG.iter().find(|c| c.id == id)
}

// ---------------------------------------------------------------------------
// Synthetic "custom" provider — escape hatch for endpoints not in the
// baked-in catalog. Same render rules as baseline anthropic / openai-chat
// providers, just with no default base_url and no model suggestions; the
// user fills everything in.
// ---------------------------------------------------------------------------

// `compatibility_warning` field stays in EndpointDef for future
// per-provider caveats, but no v1 catalog entry fills it. Earlier we
// painted a blanket "codex requires Responses" warning on every
// openai-chat endpoint; user testing showed at least some third-party
// providers do serve /v1/responses, so the warning was over-cautious.
// Re-add per-provider when we have concrete evidence a specific
// endpoint breaks.

pub fn custom() -> &'static ProviderCatalog {
    static CUSTOM: LazyLock<ProviderCatalog> = LazyLock::new(|| {
        ProviderCatalog {
        id: "custom".to_string(),
        label: "Custom".to_string(),
        icon: Some("✨".to_string()),
        homepage: None,
        endpoints: vec![
            EndpointDef {
                api_type: "anthropic".to_string(),
                default_base_url: String::new(),
                models: Vec::new(),
                capabilities: EndpointCapabilities::default(),
                compatibility_warning: None,
                auth_modes: vec![AuthModeDef {
                    mode: "api_key".to_string(),
                    label: Some("Use API key".to_string()),
                    fields: vec![FieldDef {
                        name: "api_key".to_string(),
                        label: "API key".to_string(),
                        secret: true,
                        required: true,
                        placeholder: None,
                        validate: None,
                    }],
                    render: Some(RenderRules {
                        env: btree_kv(&[
                            ("ANTHROPIC_API_KEY", "{{api_key}}"),
                            ("ANTHROPIC_BASE_URL", "{{base_url}}"),
                            ("ANTHROPIC_MODEL", "{{model}}"),
                        ]),
                        settings_files: Vec::new(),
                    }),
                    login_command: None,
                    session_file_check: None,
                }],
            },
            EndpointDef {
                api_type: "openai-responses".to_string(),
                default_base_url: String::new(),
                models: Vec::new(),
                capabilities: EndpointCapabilities {
                    reasoning_effort: true,
                },
                compatibility_warning: None,
                auth_modes: vec![AuthModeDef {
                    mode: "api_key".to_string(),
                    label: Some("Use API key".to_string()),
                    fields: vec![FieldDef {
                        name: "api_key".to_string(),
                        label: "API key".to_string(),
                        secret: true,
                        required: true,
                        placeholder: None,
                        validate: None,
                    }],
                    render: Some(RenderRules {
                        env: btree_kv(&[("OPENAI_API_KEY", "{{api_key}}")]),
                        settings_files: vec![
                            SettingsFileTemplate {
                                rel_path: "config.toml".to_string(),
                                template: "model = \"{{model}}\"\nmodel_provider = \"custom\"\nmodel_reasoning_effort = \"{{reasoning_effort}}\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"{{base_url}}\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n".to_string(),
                            },
                            SettingsFileTemplate {
                                rel_path: "auth.json".to_string(),
                                template: "{\n  \"OPENAI_API_KEY\": \"{{api_key|json}}\"\n}\n".to_string(),
                            },
                        ],
                    }),
                    login_command: None,
                    session_file_check: None,
                }],
            },
            EndpointDef {
                api_type: "openai-chat".to_string(),
                default_base_url: String::new(),
                models: Vec::new(),
                capabilities: EndpointCapabilities::default(),
                compatibility_warning: None,
                auth_modes: vec![AuthModeDef {
                    mode: "api_key".to_string(),
                    label: Some("Use API key".to_string()),
                    fields: vec![FieldDef {
                        name: "api_key".to_string(),
                        label: "API key".to_string(),
                        secret: true,
                        required: true,
                        placeholder: None,
                        validate: None,
                    }],
                    render: Some(RenderRules {
                        env: btree_kv(&[("OPENAI_API_KEY", "{{api_key}}")]),
                        settings_files: vec![
                            SettingsFileTemplate {
                                rel_path: "config.toml".to_string(),
                                template: "model = \"{{model}}\"\nmodel_provider = \"custom\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"{{base_url}}\"\nwire_api = \"chat\"\nrequires_openai_auth = true\n".to_string(),
                            },
                            SettingsFileTemplate {
                                rel_path: "auth.json".to_string(),
                                template: "{\n  \"OPENAI_API_KEY\": \"{{api_key|json}}\"\n}\n".to_string(),
                            },
                        ],
                    }),
                    login_command: None,
                    session_file_check: None,
                }],
            },
        ],
    }
    });
    &CUSTOM
}

fn btree_kv(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_catalog_parses() {
        // Touching `all()` triggers the LazyLock; the panic-on-parse-error
        // contract above means a successful call here proves all bundled
        // JSONs are well-formed.
        let entries = all();
        assert!(entries.len() >= 5);
        assert!(get("moonshot").is_some());
        assert!(get("deepseek").is_some());
        assert!(get("openrouter").is_some());
        assert!(get("minimax").is_some());
        assert!(get("zai").is_some());
        assert!(get("gemini").is_some());
        assert!(get("azure").is_some());
    }

    #[test]
    fn moonshot_supports_both_api_types() {
        let provider = get("moonshot").expect("moonshot must exist");
        let api_types: Vec<_> = provider
            .endpoints
            .iter()
            .map(|e| e.api_type.as_str())
            .collect();
        assert!(api_types.contains(&"anthropic"));
        assert!(api_types.contains(&"openai-chat"));
    }

    #[test]
    fn azure_supports_responses_only() {
        let provider = get("azure").expect("azure must exist");
        let api_types: Vec<_> = provider
            .endpoints
            .iter()
            .map(|e| e.api_type.as_str())
            .collect();
        assert_eq!(api_types, vec!["openai-responses"]);
    }

    #[test]
    fn gemini_supports_api_key_launch() {
        let provider = get("gemini").expect("gemini must exist");
        let endpoint = provider
            .endpoints
            .iter()
            .find(|e| e.api_type == "gemini")
            .expect("gemini endpoint must exist");
        let auth = endpoint
            .auth_modes
            .iter()
            .find(|a| a.mode == "api_key")
            .expect("gemini api_key auth must exist");
        let render = auth.render.as_ref().expect("api_key auth renders env");
        assert_eq!(
            render.env.get("GEMINI_API_KEY").map(String::as_str),
            Some("{{api_key}}")
        );
        assert_eq!(
            render.env.get("GEMINI_MODEL").map(String::as_str),
            Some("{{model}}")
        );
        assert_eq!(
            render.env.get("GOOGLE_GEMINI_BASE_URL").map(String::as_str),
            Some("{{base_url}}")
        );
    }
}
