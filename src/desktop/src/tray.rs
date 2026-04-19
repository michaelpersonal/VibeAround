//! System tray (Tray-First UX). Tauri 2.10 API.
//! Native menu with quick actions; "Show Window" opens the main webview window.
//! During onboarding, service-related items are disabled until `onboarding-complete` fires.

use std::sync::atomic::Ordering;

use tauri::{
    image::Image,
    menu::{Menu, MenuItemBuilder},
    tray::TrayIconBuilder,
    App, Listener, Manager, Runtime,
};

use crate::{AppTunnels, OnboardingActive};

const MAIN_WINDOW_LABEL: &str = "main";
const LOCAL_DASHBOARD_URL: &str = "http://127.0.0.1:12358/va";

/// Build the dashboard URL with the session auth token.
///
/// The SPA reads `?token=` on load, stores it in `sessionStorage`, and
/// strips the query from the address bar so it never ends up in history
/// or referer headers. Without the token, the dashboard's API requests
/// all return 401.
fn dashboard_url_with_token(base: &str) -> String {
    match common::auth::read_token_file() {
        Some(f) => format!("{base}/?token={}", f.token),
        None => base.to_string(),
    }
}

/// Build a tunnel URL with the session auth token appended.
fn tunnel_url_with_token(tunnel_url: &str) -> String {
    let Some(file) = common::auth::read_token_file() else {
        return tunnel_url.to_string();
    };
    let sep = if tunnel_url.contains('?') { '&' } else { '?' };
    format!("{tunnel_url}{sep}token={}", file.token)
}

pub fn setup<R: Runtime>(app: &App<R>) -> Result<(), Box<dyn std::error::Error>> {
    let is_onboarding = app
        .try_state::<OnboardingActive>()
        .map(|s| s.0.load(Ordering::Relaxed))
        .unwrap_or(false);

    let show_item = MenuItemBuilder::with_id("show_window", "Show Window").build(app)?;
    let open_local_item = MenuItemBuilder::with_id("open_local", "Open Local Dashboard")
        .enabled(!is_onboarding)
        .build(app)?;
    let open_tunnel_item = MenuItemBuilder::with_id("open_tunnel", "Open Tunnel")
        .enabled(false)
        .build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = Menu::with_items(app, &[&show_item, &open_local_item, &open_tunnel_item, &quit_item])?;

    // Embed the tray icon bytes at compile time so the shipped binary
    // doesn't depend on any filesystem path being present at runtime.
    //
    // Using `env!("CARGO_MANIFEST_DIR")` + `Image::from_path` as we did
    // previously bakes the *build machine's* absolute source path into
    // the binary. On every other machine `Image::from_path` returns
    // `Err`, `tray::setup` returns that error, Tauri treats setup failure
    // as fatal, and the app crashes at launch with an "application quit
    // unexpectedly" dialog.
    const TRAY_ICON_PNG: &[u8] = include_bytes!("../icons/32x32.png");
    let icon = Image::from_bytes(TRAY_ICON_PNG)?;

    // Clone items for the async watchers
    let tunnel_item_clone = open_tunnel_item.clone();
    let open_local_clone = open_local_item.clone();

    TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .tooltip("VibeAround")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show_window" => {
                if let Some(w) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "open_local" => {
                let _ = open::that(dashboard_url_with_token(LOCAL_DASHBOARD_URL));
            }
            "open_tunnel" => {
                if let Some(state) = app.try_state::<AppTunnels>() {
                    if let Some(url) = state.0.first_url() {
                        let dashboard_url = format!("{}/va", url.trim_end_matches('/'));
                        let _ = open::that(tunnel_url_with_token(&dashboard_url));
                    }
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    // Listen for onboarding-complete → re-enable menu items
    let open_local_onboard = open_local_clone.clone();
    app.listen("onboarding-complete", move |_| {
        let _ = open_local_onboard.set_enabled(true);
    });

    // Watch for tunnel state changes → enable/disable "Open Tunnel" menu item
    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        use common::state::StateSource;
        let Some(state) = app_handle.try_state::<AppTunnels>() else { return };
        let tunnels = &state.0;
        let mut rx = tunnels.subscribe_changes();
        loop {
            let has_url = tunnels.has_url();
            let _ = tunnel_item_clone.set_enabled(has_url);

            // Wait for next change notification
            if rx.recv().await.is_err() { break; }
        }
    });

    Ok(())
}
