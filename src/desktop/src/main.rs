// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod onboarding;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::{Mutex, Notify};

use onboarding::{OnboardingGate, OnboardingSessions};

/// Shared ServiceStatusManager, injected into Tauri state for tray and IPC access.
pub struct AppServiceManager(pub Arc<common::service::ServiceStatusManager>);

/// Whether the app is currently in onboarding mode (tray reads this).
pub struct OnboardingActive(pub std::sync::atomic::AtomicBool);

pub struct DaemonController {
    daemon: Arc<server::ServerDaemon>,
    dist_path: PathBuf,
    running: Mutex<Option<server::RunningDaemon>>,
}

impl DaemonController {
    pub fn new(daemon: Arc<server::ServerDaemon>, dist_path: PathBuf) -> Self {
        Self {
            daemon,
            dist_path,
            running: Mutex::new(None),
        }
    }

    pub async fn start(&self) -> Result<(), String> {
        let mut running = self.running.lock().await;
        if running.is_some() {
            return Ok(());
        }

        let daemon = self
            .daemon
            .start_background(self.dist_path.clone())
            .await
            .map_err(|e| e.to_string())?;
        *running = Some(daemon);
        Ok(())
    }

    pub async fn stop(&self) {
        let mut running = self.running.lock().await;
        if let Some(daemon) = running.take() {
            daemon.stop().await;
        }
    }

    pub async fn restart(&self) -> Result<(), String> {
        self.stop().await;
        self.start().await
    }
}

pub async fn start_daemon<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let controller = app
        .try_state::<DaemonController>()
        .ok_or_else(|| "daemon controller state is missing".to_string())?;
    controller.start().await
}

pub async fn stop_daemon<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let controller = app
        .try_state::<DaemonController>()
        .ok_or_else(|| "daemon controller state is missing".to_string())?;
    controller.stop().await;
    Ok(())
}

pub async fn restart_daemon<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let controller = app
        .try_state::<DaemonController>()
        .ok_or_else(|| "daemon controller state is missing".to_string())?;
    controller.restart().await
}

fn main() {
    // Fast-path: if our port is already in use, exit immediately before
    // allocating Tauri resources. tauri_plugin_single_instance (below) is the
    // real guard, but this avoids a full Tauri init just to discover the duplicate.
    let port = common::config::DEFAULT_PORT;
    if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
        eprintln!(
            "[VibeAround] Another instance is already running (port {} in use). Exiting.",
            port
        );
        std::process::exit(0);
    }

    let daemon = Arc::new(server::ServerDaemon::new(port));
    let services = daemon.services();

    let onboarding_needed = onboarding::needs_onboarding();
    let gate = Arc::new(Notify::new());
    let dist_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/dist");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            eprintln!("[VibeAround] ⚠️  Another instance tried to start, focusing existing window");
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .manage(AppServiceManager(services))
        .manage(DaemonController::new(Arc::clone(&daemon), dist_path.clone()))
        .manage(OnboardingGate { notify: Arc::clone(&gate) })
        .manage(OnboardingSessions {
            plugin_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        })
        .manage(OnboardingActive(std::sync::atomic::AtomicBool::new(onboarding_needed)))
        .invoke_handler(tauri::generate_handler![
            onboarding::get_settings,
            onboarding::list_channel_plugins,
            onboarding::save_settings,
            onboarding::install_plugin,
            onboarding::check_plugin_status,
            onboarding::plugin_auth_start,
            onboarding::plugin_auth_wait,
            onboarding::plugin_auth_cancel,
            onboarding::finish_onboarding,
            onboarding::list_agents,
            onboarding::list_tunnels,
            onboarding::list_plugin_registry,
        ])
        .setup(move |app| {
            tray::setup(app)?;

            // Show the main window on startup
            if let Some(w) = app.get_webview_window("main") {
                if onboarding_needed {
                    // Navigate to onboarding page
                    let _ = w.eval("window.location.replace('/onboarding')");
                }
                let _ = w.show();
                let _ = w.set_focus();
            }

            // Start the daemon — immediately if no onboarding needed,
            // otherwise wait for the onboarding gate signal.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if onboarding_needed {
                    eprintln!("[VibeAround] Waiting for onboarding to complete…");
                    gate.notified().await;
                    eprintln!("[VibeAround] Onboarding complete, starting daemon…");

                    // Mark onboarding as done for tray
                    if let Some(state) = app_handle.try_state::<OnboardingActive>() {
                        state.0.store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                }

                if let Err(e) = start_daemon(&app_handle).await {
                    eprintln!("[VibeAround] Daemon error: {}", e);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running VibeAround");
}
