//! System tray (Tray-First UX). Tauri 2.10 API.
//! Native menu with quick actions; "Show Window" opens the main webview window.
//! During onboarding, service-related items are disabled until `onboarding-complete` fires.

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use tauri::{
    image::Image,
    menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    tray::{TrayIcon, TrayIconBuilder},
    App, AppHandle, Listener, Manager, Runtime,
};

use crate::{AppTunnels, OnboardingActive};

pub(crate) const LAUNCH_CONFIG_CHANGED_EVENT: &str = "launch-config-changed";

const TRAY_ID: &str = "main";
const MAIN_WINDOW_LABEL: &str = "main";
const LOCAL_DASHBOARD_URL: &str = "http://127.0.0.1:12358/va";
const MENU_LAUNCH_DEFAULT: &str = "launch_default";
const MENU_SHOW_WINDOW: &str = "show_window";
const MENU_OPEN_LOCAL: &str = "open_local";
const MENU_OPEN_TUNNEL: &str = "open_tunnel";
const MENU_QUIT: &str = "quit";
const MENU_LAUNCH_DIRECT_PREFIX: &str = "launch_direct:";
const MENU_LAUNCH_PROFILE_PREFIX: &str = "launch_profile:";
const SETTINGS_UI_LOCALE: &str = "ui_locale";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum UiLocale {
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl UiLocale {
    fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" | "en-us" | "en_us" => Some(Self::En),
            "zh" | "zh-cn" | "zh_cn" | "zh-hans" | "zh_hans" => Some(Self::ZhCn),
            _ => None,
        }
    }

    fn as_settings_value(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
        }
    }

    fn text(self, key: TrayText) -> &'static str {
        match self {
            Self::En => key.en(),
            Self::ZhCn => match key {
                TrayText::ShowWindow => "显示窗口",
                TrayText::QuickLaunch => "快速启动",
                TrayText::LaunchWithoutProfile => "不使用 Profile 启动",
                TrayText::OpenDashboard => "打开控制台",
                TrayText::OpenTunnel => "打开隧道",
                TrayText::Quit => "退出",
            },
        }
    }
}

#[derive(Clone, Copy)]
enum TrayText {
    ShowWindow,
    QuickLaunch,
    LaunchWithoutProfile,
    OpenDashboard,
    OpenTunnel,
    Quit,
}

impl TrayText {
    fn en(self) -> &'static str {
        match self {
            Self::ShowWindow => "Show Window",
            Self::QuickLaunch => "Quick Launch",
            Self::LaunchWithoutProfile => "Launch Without Profile",
            Self::OpenDashboard => "Open Dashboard",
            Self::OpenTunnel => "Open Tunnel",
            Self::Quit => "Quit",
        }
    }
}

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
    let app_handle = app.handle().clone();
    let menu = build_menu(&app_handle)?;

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

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .tooltip("VibeAround")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            MENU_LAUNCH_DEFAULT => {
                if let Err(e) = crate::profiles::profiles_launch_default() {
                    tracing::warn!("[tray] failed to launch default agent: {}", e);
                }
            }
            MENU_SHOW_WINDOW => {
                if let Some(w) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            MENU_OPEN_LOCAL => {
                let _ = open::that(dashboard_url_with_token(LOCAL_DASHBOARD_URL));
            }
            MENU_OPEN_TUNNEL => {
                if let Some(state) = app.try_state::<AppTunnels>() {
                    if let Some(url) = state.0.first_url() {
                        let dashboard_url = format!("{}/va", url.trim_end_matches('/'));
                        let _ = open::that(tunnel_url_with_token(&dashboard_url));
                    }
                }
            }
            MENU_QUIT => {
                app.exit(0);
            }
            id if id.starts_with(MENU_LAUNCH_DIRECT_PREFIX) => {
                if let Err(e) = handle_direct_launch_menu(id) {
                    tracing::warn!("[tray] failed to direct launch menu item '{}': {}", id, e);
                }
            }
            id if id.starts_with(MENU_LAUNCH_PROFILE_PREFIX) => {
                if let Err(e) = handle_profile_launch_menu(id) {
                    tracing::warn!("[tray] failed to launch profile menu item '{}': {}", id, e);
                }
            }
            _ => {}
        })
        .build(app)?;

    // Listen for onboarding-complete -> rebuild menu items with launch enabled.
    let app_handle_onboarding = app_handle.clone();
    app.listen("onboarding-complete", move |_| {
        if let Err(e) = rebuild_menu(&app_handle_onboarding) {
            tracing::warn!("[tray] failed to rebuild after onboarding: {}", e);
        }
    });

    // Profile/default changes alter the launch menu tree.
    let app_handle_launch_config = app_handle.clone();
    app.listen(LAUNCH_CONFIG_CHANGED_EVENT, move |_| {
        if let Err(e) = rebuild_menu(&app_handle_launch_config) {
            tracing::warn!("[tray] failed to rebuild launch menu: {}", e);
        }
    });

    // Watch for tunnel state changes -> rebuild the "Open Tunnel" enabled state.
    tauri::async_runtime::spawn(async move {
        use common::state::StateSource;
        let Some(state) = app_handle.try_state::<AppTunnels>() else {
            return;
        };
        let tunnels = &state.0;
        let mut rx = tunnels.subscribe_changes();
        loop {
            if let Err(e) = rebuild_menu(&app_handle) {
                tracing::warn!("[tray] failed to rebuild after tunnel change: {}", e);
            }

            // Wait for next change notification
            if rx.recv().await.is_err() {
                break;
            }
        }
    });

    Ok(())
}

fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let locale = current_locale();
    let is_onboarding = is_onboarding(app);
    let launch_enabled = !is_onboarding;
    let has_tunnel_url = app
        .try_state::<AppTunnels>()
        .map(|state| state.0.has_url())
        .unwrap_or(false);

    let show_item =
        MenuItemBuilder::with_id(MENU_SHOW_WINDOW, locale.text(TrayText::ShowWindow)).build(app)?;
    let launch_default_item =
        MenuItemBuilder::with_id(MENU_LAUNCH_DEFAULT, locale.text(TrayText::QuickLaunch))
            .enabled(launch_enabled)
            .build(app)?;
    let direct_launch_menu = build_direct_launch_submenu(app, launch_enabled, locale)?;
    let profile_menus = build_profile_submenus(app, launch_enabled)?;
    let open_local_item =
        MenuItemBuilder::with_id(MENU_OPEN_LOCAL, locale.text(TrayText::OpenDashboard))
            .enabled(launch_enabled)
            .build(app)?;
    let open_tunnel_item =
        MenuItemBuilder::with_id(MENU_OPEN_TUNNEL, locale.text(TrayText::OpenTunnel))
            .enabled(launch_enabled && has_tunnel_url)
            .build(app)?;
    let quit_item = MenuItemBuilder::with_id(MENU_QUIT, locale.text(TrayText::Quit)).build(app)?;

    let mut builder = MenuBuilder::new(app)
        .item(&show_item)
        .separator()
        .item(&launch_default_item)
        .item(&direct_launch_menu);
    for profile_menu in &profile_menus {
        builder = builder.item(profile_menu);
    }

    builder
        .separator()
        .item(&open_local_item)
        .item(&open_tunnel_item)
        .separator()
        .item(&quit_item)
        .build()
}

fn build_direct_launch_submenu<R: Runtime>(
    app: &AppHandle<R>,
    launch_enabled: bool,
    locale: UiLocale,
) -> tauri::Result<tauri::menu::Submenu<R>> {
    let mut builder = SubmenuBuilder::with_id(
        app,
        "direct_launch",
        locale.text(TrayText::LaunchWithoutProfile),
    )
    .enabled(launch_enabled);

    for agent in common::resources::AGENTS.iter() {
        let item = MenuItemBuilder::with_id(
            format!("{}{}", MENU_LAUNCH_DIRECT_PREFIX, agent.id),
            menu_text(&agent.display_name),
        )
        .enabled(launch_enabled)
        .build(app)?;
        builder = builder.item(&item);
    }

    builder.build()
}

fn build_profile_submenus<R: Runtime>(
    app: &AppHandle<R>,
    launch_enabled: bool,
) -> tauri::Result<Vec<tauri::menu::Submenu<R>>> {
    let profiles = crate::profiles::ordered_profiles();

    if profiles.is_empty() {
        return Ok(Vec::new());
    }

    let provider_counts = profile_provider_counts(&profiles);
    let mut out = Vec::new();
    for profile in &profiles {
        let targets = common::profiles::runtime::launch_targets_for_api_types(&profile.api_types);
        if targets.is_empty() {
            continue;
        }

        let title = profile_menu_title(profile, &provider_counts);
        let mut profile_builder = SubmenuBuilder::with_id(
            app,
            format!("launch_profile_menu:{}", profile.id),
            menu_text(&title),
        )
        .enabled(launch_enabled);

        for (agent_id, label, _) in targets {
            let item = MenuItemBuilder::with_id(
                format!("{}{}:{}", MENU_LAUNCH_PROFILE_PREFIX, profile.id, agent_id),
                menu_text(label),
            )
            .enabled(launch_enabled)
            .build(app)?;
            profile_builder = profile_builder.item(&item);
        }

        let profile_menu = profile_builder.build()?;
        out.push(profile_menu);
    }

    Ok(out)
}

fn rebuild_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    if let Some(tray) = tray_icon(app) {
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

pub(crate) fn set_ui_locale<R: Runtime>(app: &AppHandle<R>, locale: &str) -> Result<(), String> {
    let locale =
        UiLocale::from_str(locale).ok_or_else(|| format!("unsupported locale: {locale}"))?;
    common::config::update_settings_json(|root| {
        if !root.is_object() {
            *root = serde_json::json!({});
        }
        if let Some(obj) = root.as_object_mut() {
            obj.insert(
                SETTINGS_UI_LOCALE.to_string(),
                serde_json::Value::String(locale.as_settings_value().to_string()),
            );
        }
    })?;
    rebuild_menu(app).map_err(|e| e.to_string())
}

fn tray_icon<R: Runtime>(app: &AppHandle<R>) -> Option<TrayIcon<R>> {
    app.tray_by_id(TRAY_ID)
}

fn is_onboarding<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<OnboardingActive>()
        .map(|s| s.0.load(Ordering::Relaxed))
        .unwrap_or(false)
}

fn current_locale() -> UiLocale {
    let path = common::config::data_dir().join("settings.json");
    let Ok(data) = std::fs::read_to_string(path) else {
        return UiLocale::En;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&data) else {
        return UiLocale::En;
    };
    root.get(SETTINGS_UI_LOCALE)
        .and_then(|value| value.as_str())
        .and_then(UiLocale::from_str)
        .unwrap_or(UiLocale::En)
}

fn handle_profile_launch_menu(menu_id: &str) -> Result<(), String> {
    let payload = menu_id
        .strip_prefix(MENU_LAUNCH_PROFILE_PREFIX)
        .ok_or_else(|| format!("invalid profile launch menu id: {menu_id}"))?;
    let (profile_id, agent_id) = payload
        .split_once(':')
        .ok_or_else(|| format!("invalid profile launch menu id: {menu_id}"))?;
    crate::profiles::profiles_launch(profile_id.to_string(), agent_id.to_string())
}

fn handle_direct_launch_menu(menu_id: &str) -> Result<(), String> {
    let agent_id = menu_id
        .strip_prefix(MENU_LAUNCH_DIRECT_PREFIX)
        .ok_or_else(|| format!("invalid direct launch menu id: {menu_id}"))?;
    crate::profiles::profiles_launch_direct(agent_id.to_string())
}

fn profile_provider_counts(
    profiles: &[common::profiles::ProfileDef],
) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for profile in profiles {
        *counts.entry(profile.provider.clone()).or_insert(0) += 1;
    }
    counts
}

fn profile_menu_title(
    profile: &common::profiles::ProfileDef,
    provider_counts: &std::collections::BTreeMap<String, usize>,
) -> String {
    let provider = common::profiles::catalog::get(&profile.provider);
    let provider_label = provider
        .map(|catalog| catalog.label.as_str())
        .unwrap_or(profile.provider.as_str());
    let needs_profile_label = provider_counts
        .get(&profile.provider)
        .copied()
        .unwrap_or_default()
        > 1
        || profile.provider == "custom";
    if needs_profile_label {
        format!("{} - {}", provider_label, profile.label)
    } else {
        provider_label.to_string()
    }
}

fn menu_text(text: &str) -> String {
    text.replace('&', "&&")
}
