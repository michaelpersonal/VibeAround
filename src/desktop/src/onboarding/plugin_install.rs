//! Plugin installation: git clone, npm install/build, and post-install verification.

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use common::{config, plugins};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPluginRequest {
    pub plugin_id: String,
    pub github_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPluginResponse {
    pub success: bool,
    pub message: String,
    /// The plugin ID as declared in the installed plugin.json (may differ from the requested id).
    pub actual_plugin_id: Option<String>,
}

/// Invoke npm via `node npm-cli.js`.
///
/// On Windows, `npm` is a `.cmd` batch script that `Command::new("npm")` cannot
/// spawn directly. Locating npm-cli.js next to `node` and calling it via node
/// works cross-platform without any PATH or shell workarounds.
pub(super) async fn npm_command(
    args: &[&str],
    cwd: &std::path::Path,
) -> std::io::Result<std::process::Output> {
    let node_info = common::process::env::command("node")
        .args(["-p", "process.execPath"])
        .output()
        .await?;
    let node_exec = String::from_utf8_lossy(&node_info.stdout)
        .trim()
        .to_string();
    let node_dir = std::path::Path::new(&node_exec).parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cannot determine node install directory",
        )
    })?;

    // Try multiple known locations for npm-cli.js:
    // 1. Next to node binary (nvm, volta, default installs)
    // 2. Homebrew global lib (brew install node on macOS)
    // 3. Homebrew prefix /opt/homebrew or /usr/local
    let candidates = [
        node_dir
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js"),
        node_dir
            .join("../lib/node_modules/npm/bin/npm-cli.js")
            .into(),
        std::path::PathBuf::from("/opt/homebrew/lib/node_modules/npm/bin/npm-cli.js"),
        std::path::PathBuf::from("/usr/local/lib/node_modules/npm/bin/npm-cli.js"),
    ];
    let npm_cli = candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "npm-cli.js not found in any of: {:?} — is npm installed with Node.js?",
                    candidates
                ),
            )
        })?;

    let mut node_args: Vec<String> = vec![npm_cli.to_string_lossy().to_string()];
    node_args.extend(args.iter().map(|s| s.to_string()));

    tracing::info!(
        "[npm_command] node {} {}",
        npm_cli.display(),
        args.join(" ")
    );
    common::process::env::command("node")
        .args(&node_args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
}

#[tauri::command]
pub async fn install_plugin(
    request: InstallPluginRequest,
) -> Result<InstallPluginResponse, String> {
    run_install_inner(request).await.map_err(|e| e.to_string())
}

/// Internal implementation — uses anyhow for ergonomic error chaining.
/// Also callable from the onboarding install orchestrator in mod.rs.
pub(super) async fn run_install_inner(
    request: InstallPluginRequest,
) -> anyhow::Result<InstallPluginResponse> {
    let plugins_dir = config::data_dir().join("plugins");
    let target_dir = plugins_dir.join(&request.plugin_id);

    std::fs::create_dir_all(&plugins_dir).context("creating plugins directory")?;

    // If a previous install left a partial directory (no dist/), wipe it for a clean clone.
    let needs_clone = if target_dir.exists() {
        if target_dir.join("dist").exists() {
            tracing::info!(
                "[install_plugin] {} already built, skipping clone",
                request.plugin_id
            );
            false
        } else {
            tracing::info!(
                "[install_plugin] {} has no dist (stale install), re-cloning",
                request.plugin_id
            );
            std::fs::remove_dir_all(&target_dir).context("removing stale plugin directory")?;
            true
        }
    } else {
        true
    };

    if needs_clone {
        tracing::info!(
            "[install_plugin] cloning {} → {:?}",
            request.github_url,
            target_dir
        );
        let output = tokio::process::Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                &request.github_url,
                &target_dir.to_string_lossy(),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .context("git clone")?;
        if !output.status.success() {
            bail!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    tracing::info!("[install_plugin] npm install in {:?}", target_dir);
    let output = npm_command(&["install"], &target_dir)
        .await
        .context("npm install")?;
    if !output.status.success() {
        bail!(
            "npm install failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    tracing::info!("[install_plugin] npm run build in {:?}", target_dir);
    let output = npm_command(&["run", "build"], &target_dir)
        .await
        .context("npm run build")?;
    if !output.status.success() {
        bail!(
            "npm run build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Build must produce dist/main.js — its absence means tsc had silent errors.
    let main_script = target_dir.join("dist").join("main.js");
    if !main_script.exists() {
        bail!(
            "build succeeded but dist/main.js was not produced (tsc may have emitted errors).\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let actual_id = match plugins::channel::find(&request.plugin_id) {
        Some(p) => {
            tracing::info!(
                "[install_plugin] {} discoverable (manifest id='{}')",
                request.plugin_id,
                p.manifest.id
            );
            Some(p.manifest.id.clone())
        }
        None => {
            // Built but not discovered — likely an ID mismatch. Surface the manifest id for the UI.
            let fallback_id = std::fs::read_to_string(target_dir.join("plugin.json"))
                .ok()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(String::from));
            tracing::info!(
                "[install_plugin] WARNING: {} built but not discoverable (manifest id={:?})",
                request.plugin_id,
                fallback_id
            );
            fallback_id
        }
    };

    Ok(InstallPluginResponse {
        success: true,
        message: format!("Plugin '{}' installed successfully", request.plugin_id),
        actual_plugin_id: actual_id,
    })
}

#[tauri::command]
pub fn check_plugin_status(plugin_id: String) -> String {
    // Check both user plugins dir (~/.vibearound/plugins/) and project plugins dir (src/plugins/)
    // via the discovery system which searches both paths.
    if plugins::channel::find(&plugin_id).is_some() {
        return "ready".to_string();
    }

    let target_dir = config::data_dir().join("plugins").join(&plugin_id);
    if !target_dir.join("plugin.json").exists() {
        return "not_installed".to_string();
    }
    if !target_dir.join("dist").join("main.js").exists() {
        return "installed_not_built".to_string();
    }
    "ready".to_string()
}
