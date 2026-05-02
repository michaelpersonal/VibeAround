//! Runtime helpers for applying rendered profiles to launched CLIs.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};

use super::catalog::{self, ProviderCatalog};
use super::render::{render, ConfigEnvTarget, RenderedProfile};
use super::schema::ProfileDef;
use crate::{auth, config};

pub fn render_for_launch(
    profile: &ProfileDef,
    launch_target: &str,
) -> anyhow::Result<RenderedProfile> {
    let provider = catalog::get(&profile.provider)
        .ok_or_else(|| anyhow!("unknown provider '{}'", profile.provider))?;
    let api_type = api_type_for_launch_target(profile, provider, launch_target)?;
    render(profile, api_type, launch_target, provider)
}

pub fn env_for_launch(
    profile: &ProfileDef,
    launch_target: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    let rendered = render_for_launch(profile, launch_target)?;
    materialize_env(&profile.id, rendered)
}

pub fn materialize_env(
    profile_id: &str,
    rendered: RenderedProfile,
) -> anyhow::Result<Vec<(String, String)>> {
    let mut env = rendered.env.clone();
    let Some(target) = rendered.config_env else {
        return Ok(env);
    };

    let dir = profile_state_dir(profile_id);
    for sf in &rendered.settings_files {
        materialize_settings_file(&dir, &sf.rel_path, &sf.contents)?;
    }
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

    Ok(env)
}

pub fn launch_targets_for_api_types(
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

pub fn agent_id_for(launch_target: &str) -> anyhow::Result<&'static str> {
    match launch_target {
        "claude" => Ok("claude"),
        "codex" => Ok("codex"),
        "gemini" => Ok("gemini"),
        "opencode" => Ok("opencode"),
        other => bail!("unsupported launch target: '{}'", other),
    }
}

pub fn api_type_for_launch_target<'a>(
    profile: &'a ProfileDef,
    provider: &'a ProviderCatalog,
    launch_target: &str,
) -> anyhow::Result<&'a str> {
    let candidates: &[&str] = match launch_target {
        "claude" => &["anthropic"],
        "codex" => &["openai-responses", "openai-chat"],
        "gemini" => &["gemini"],
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

pub fn profile_state_dir(id: &str) -> PathBuf {
    config::data_dir().join("profile-state").join(id)
}

fn materialize_settings_file(dir: &Path, rel_path: &str, contents: &str) -> anyhow::Result<()> {
    let target = dir.join(rel_path);

    // Defense in depth: render::validate_rel_path rejects `..`, but this
    // catches catalog paths whose parent resolves through a symlink.
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
