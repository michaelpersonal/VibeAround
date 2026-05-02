//! Media relocation — move cached files from staging to the session-scoped
//! workspace path and rewrite the URI on the resource-link content block.
//!
//! Content blocks arriving in a prompt can reference files that were
//! uploaded into the global `.cache/` staging dir. Before handing them to
//! the agent we move each file into a session-scoped cache under the
//! workspace so the agent sees a stable URI tied to the live session.

use agent_client_protocol as acp;

use crate::routing::RouteKey;
use crate::config;

/// Scan content blocks for `resource_link` with `file://` URIs under the
/// global `.cache/` staging dir. Move each file to the workspace session
/// path and update the URI.
pub(super) async fn relocate_cached_media(
    mut blocks: Vec<acp::ContentBlock>,
    route: &RouteKey,
    agent_kind: &str,
    session_id: &str,
) -> Vec<acp::ContentBlock> {
    let cache_dir = config::data_dir().join(".cache");
    let cache_prefix = format!("file://{}/", cache_dir.to_string_lossy());

    let workspace_cache = config::data_dir()
        .join("workspaces")
        .join(".cache")
        .join(&*route.channel_kind)
        .join(&*route.chat_id)
        .join(agent_kind)
        .join(session_id);

    for block in blocks.iter_mut() {
        if let acp::ContentBlock::ResourceLink(ref mut rl) = block {
            let uri = rl.uri.to_string();
            if !uri.starts_with(&cache_prefix) {
                continue;
            }
            let src_path = uri.strip_prefix("file://").unwrap_or(&uri);
            let src = std::path::Path::new(src_path);
            if !src.exists() {
                tracing::info!("[Conversation] relocate: source not found {}", src.display());
                continue;
            }
            let file_name = src
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let dest = workspace_cache.join(&file_name);

            if let Err(e) = tokio::fs::create_dir_all(&workspace_cache).await {
                tracing::info!(
                    "[Conversation] relocate: mkdir failed {}: {}",
                    workspace_cache.display(),
                    e
                );
                continue;
            }
            if let Err(e) = tokio::fs::rename(src, &dest).await {
                // rename may fail across filesystems; fall back to copy+remove
                if let Err(e2) = tokio::fs::copy(src, &dest).await {
                    tracing::info!(
                        "[Conversation] relocate: move failed {} -> {}: rename={}, copy={}",
                        src.display(),
                        dest.display(),
                        e,
                        e2
                    );
                    continue;
                }
                let _ = tokio::fs::remove_file(src).await;
            }

            let new_uri = format!("file://{}", dest.to_string_lossy());
            tracing::info!("[Conversation] relocate: {} -> {}", src.display(), dest.display());
            rl.uri = new_uri;
        }
    }

    blocks
}
