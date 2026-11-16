#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ble;
mod cloud;
mod core;
mod i18n;
mod models;
mod notify;
mod settings;
mod tray;

use crate::core::{now_ms, Core, Shared, Snapshot};
use crate::settings::Settings;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Rect, Runtime, WebviewWindow, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const TRAY_ID: &str = "main";

struct AppState {
    core: Shared,
    /// When the popup was last hidden by losing focus (to ignore the same click re-opening it).
    last_blur_hide: AtomicU64,
    last_look: Mutex<Option<tray::TrayLook>>,
    menu_lang: Mutex<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    settings: Settings,
    has_password: bool,
    version: String,
    platform: String,
}

fn settings_view<R: Runtime>(app: &AppHandle<R>, core: &Shared) -> SettingsView {
    let s = core.settings();
    SettingsView {
        has_password: settings::get_password(&s.cloud_email).is_some(),
        settings: s,
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS.to_string(),
    }
}

#[tauri::command]
fn get_snapshot(state: tauri::State<AppState>) -> Snapshot {
    state.core.snapshot()
}

#[tauri::command]
fn get_settings(app: AppHandle, state: tauri::State<AppState>) -> SettingsView {
    settings_view(&app, &state.core)
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    state: tauri::State<AppState>,
    mut settings: Settings,
    password: Option<String>,
) -> Result<SettingsView, String> {
    let core = &state.core;
    settings.sanitize();
    let old = core.settings();
    // Managed by the backend only; the popup may hold an older copy.
    settings.last_seen_sn = old.last_seen_sn.clone();

    let mut password_changed = false;
    if let Some(p) = password.filter(|p| !p.is_empty()) {
        if settings.cloud_email.is_empty() {
            return Err("email".into());
        }
        settings::set_password(&settings.cloud_email, &p)?;
        password_changed = true;
    }
    if !old.cloud_email.is_empty() && old.cloud_email != settings.cloud_email {
        settings::delete_password(&old.cloud_email);
    }

    settings::save(&core.settings_path, &settings)?;
    *core.settings.write().unwrap() = settings.clone();

    if settings.autostart != old.autostart {
        let al = app.autolaunch();
        let r = if settings.autostart { al.enable() } else { al.disable() };
        if let Err(e) = r {
            log::warn!("autostart: {e}");
        }
    }
    if settings.ble_enabled != old.ble_enabled {
        core.ble_restart.notify_one();
    }
    let cloud_changed = password_changed
        || settings.cloud_enabled != old.cloud_enabled
        || settings.cloud_email != old.cloud_email
        || settings.cloud_region != old.cloud_region
        || settings.device_sn != old.device_sn;
    if cloud_changed {
        core.cloud_restart.notify_one();
    }
    if settings.device_sn != old.device_sn {
        core.notify.lock().unwrap().initialized = false;
    }
    core.mark_changed();
    Ok(settings_view(&app, core))
}

#[tauri::command]
fn set_popup_height(window: WebviewWindow, height: f64) {
    let h = height.clamp(120.0, 640.0);
    if let Ok(size) = window.inner_size() {
        let scale = window.scale_factor().unwrap_or(1.0);
        let w = size.width as f64 / scale;
        let current = size.height as f64 / scale;
        if (current - h).abs() > 0.5 {
            // When the popup sits in the lower half of the screen (taskbar at the bottom),
            // keep its bottom edge in place so it doesn't slide under the taskbar.
            let anchored_bottom = match (window.outer_position(), window.current_monitor()) {
                (Ok(pos), Ok(Some(m))) => {
                    let mid = m.position().y as f64 + m.size().height as f64 / 2.0;
                    (pos.y as f64 + size.height as f64 / 2.0) > mid
                }
                _ => false,
            };
            let old_pos = window.outer_position().ok();
            let _ = window.set_size(tauri::LogicalSize::new(w, h));
            if let (true, Some(p)) = (anchored_bottom, old_pos) {
                let delta = ((h - current) * scale).round() as i32;
                let _ = window.set_position(tauri::PhysicalPosition::new(p.x, p.y - delta));
            }
        }
    }
}

#[tauri::command]
fn hide_popup(window: WebviewWindow) {
    let _ = window.hide();
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

fn show_popup<R: Runtime>(app: &AppHandle<R>, rect: Option<Rect>, open_settings: bool) {
    let Some(win) = tray::popup(app) else { return };
    tray::place_popup(&win, rect);
    let _ = win.show();
    let _ = win.set_focus();
    let _ = app.emit("popup-shown", open_settings);
}

fn toggle_popup<R: Runtime>(app: &AppHandle<R>, rect: Rect) {
    let Some(win) = tray::popup(app) else { return };
    let state = app.state::<AppState>();
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }
    // The click that took focus away already hid it: treat as "close".
    if now_ms().saturating_sub(state.last_blur_hide.load(Ordering::Relaxed)) < 350 {
        return;
    }
    show_popup(app, Some(rect), false);
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, lang: &str) -> tauri::Result<Menu<R>> {
    let t = i18n::get(lang);
    let open = MenuItem::with_id(app, "open", t.open, true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", t.settings, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", t.quit, true, None::<&str>)?;
    Menu::with_items(app, &[&open, &settings, &quit])
}

fn apply_tray<R: Runtime>(app: &AppHandle<R>, tray_icon: &TrayIcon<R>, snap: &Snapshot) {
    let state = app.state::<AppState>();
    let low = state.core.settings().low_threshold;
    let look = tray::look(snap, low);
    let mut last = state.last_look.lock().unwrap();
    if last.as_ref() != Some(&look) {
        if cfg!(target_os = "macos") {
            let _ = tray_icon.set_icon(Some(tray::render_battery_template(look.fill)));
            let _ = tray_icon.set_icon_as_template(true);
            let title = match snap.soc {
                Some(soc) => format!("{soc}%"),
                None => "–".to_string(),
            };
            let _ = tray_icon.set_title(Some(title));
        } else {
            let _ = tray_icon.set_icon(Some(tray::render_number(&look)));
        }
        *last = Some(look);
    }
    let _ = tray_icon.set_tooltip(Some(tray::tooltip(snap, &snap.lang)));

    let mut menu_lang = state.menu_lang.lock().unwrap();
    if *menu_lang != snap.lang {
        if let Ok(menu) = build_menu(app, &snap.lang) {
            let _ = tray_icon.set_menu(Some(menu));
            *menu_lang = snap.lang.clone();
        }
    }
}

async fn change_loop(app: AppHandle) {
    let core = app.state::<AppState>().core.clone();
    let mut last: Option<Snapshot> = None;
    let mut periodic = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = core.changed.notified() => {
                // Coalesce bursts of adverts.
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            _ = periodic.tick() => {}
        }
        let snap = core.snapshot();
        if let Some(tray_icon) = app.tray_by_id(TRAY_ID) {
            apply_tray(&app, &tray_icon, &snap);
        }
        if let Some(kind) = notify::evaluate(&core, &snap) {
            notify::send(&app, &snap, kind);
        }
        if last.as_ref() != Some(&snap) {
            let _ = app.emit("snapshot", &snap);
            last = Some(snap);
        }
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_popup(app, None, false);
        }))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_settings,
            save_settings,
            set_popup_height,
            hide_popup,
            quit_app
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            notify::register_app_id(&app.config().identifier, "Station Battery");

            let dir = app.path().app_config_dir()?;
            let path = dir.join("settings.json");
            let s = settings::load(&path);
            let lang = s.resolved_lang().to_string();
            // Keep the autostart entry pointing at the current exe (portable app may move).
            if s.autostart {
                let _ = app.autolaunch().enable();
            }
            let core = Core::new(s, path);
            app.manage(AppState {
                core: core.clone(),
                last_blur_hide: AtomicU64::new(0),
                last_look: Mutex::new(None),
                menu_lang: Mutex::new(lang.clone()),
            });

            let snap = core.snapshot();
            let look = tray::look(&snap, core.settings().low_threshold);
            let icon = if cfg!(target_os = "macos") {
                tray::render_battery_template(None)
            } else {
                tray::render_number(&look)
            };
            let mut builder = TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .tooltip(tray::tooltip(&snap, &lang))
                .menu(&build_menu(app.handle(), &lang)?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_popup(app, None, false),
                    "settings" => show_popup(app, None, true),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray_icon, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_popup(tray_icon.app_handle(), rect);
                    }
                });
            if cfg!(target_os = "macos") {
                builder = builder.icon_as_template(true).title("–");
            }
            builder.build(app)?;

            if let Some(win) = tray::popup(app.handle()) {
                let handle = app.handle().clone();
                win.on_window_event(move |e| {
                    if let WindowEvent::Focused(false) = e {
                        if let Some(w) = tray::popup(&handle) {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                                handle
                                    .state::<AppState>()
                                    .last_blur_hide
                                    .store(now_ms(), Ordering::Relaxed);
                            }
                        }
                    }
                });
            }

            tauri::async_runtime::spawn(ble::run(core.clone()));
            tauri::async_runtime::spawn(cloud::run(core.clone()));
            tauri::async_runtime::spawn(change_loop(app.handle().clone()));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building app")
        .run(|_app, event| {
            // Tray app: closing the popup must not quit.
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
