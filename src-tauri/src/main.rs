#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ble;
mod cloud;
mod core;
mod i18n;
mod models;
mod notify;
mod platform;
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
    /// What the tray currently shows (look + voltage unit), to skip redundant redraws.
    last_look: Mutex<Option<(tray::TrayLook, &'static str)>>,
    menu_lang: Mutex<String>,
    /// Where the visible popup is pinned (set each time it is shown).
    anchor: Mutex<Option<tray::Anchor>>,
    /// Last height requested by the popup (logical px); starts at the configured size.
    popup_height: Mutex<f64>,
    /// A show request waiting for the page to lay itself out: (request id, tray rect).
    pending_show: Mutex<Option<(u64, Option<Rect>)>>,
    show_seq: AtomicU64,
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
        has_password: core.has_password(&s.cloud_email),
        settings: s,
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS.to_string(),
    }
}

#[tauri::command]
fn get_platform() -> platform::PlatformInfo {
    platform::info()
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    if let Some(p) = tray::popup(&app) {
        let _ = p.hide();
    }
    open_settings_window(&app);
}

/// Always creates the window from its own thread: on Windows, building a webview window
/// inside a synchronous command or an event handler (tray menu, single-instance callback)
/// deadlocks the main thread.
fn open_settings_window<R: Runtime>(app: &AppHandle<R>) {
    let lang = app.state::<AppState>().core.settings().resolved_lang();
    let app = app.clone();
    std::thread::spawn(move || platform::open_settings(&app, i18n::get(lang).settings_title));
}

/// The settings page calls this once it has rendered, so the window never flashes empty.
#[tauri::command]
fn reveal_window(window: WebviewWindow) {
    let _ = window.show();
    let _ = window.set_focus();
}

/// macOS settings: the window height follows the selected tab.
#[tauri::command]
fn set_window_height(window: WebviewWindow, height: f64) {
    let h = height.clamp(160.0, 900.0);
    let Ok(size) = window.inner_size() else { return };
    let scale = window.scale_factor().unwrap_or(1.0);
    let w = size.width as f64 / scale;
    let _ = window.set_size(tauri::LogicalSize::new(w, h));
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
        core.set_has_password(&settings.cloud_email, true);
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
    if settings.lang != old.lang {
        if let Some(w) = app.get_webview_window("settings") {
            let _ = w.set_title(i18n::get(settings.resolved_lang()).settings_title);
        }
    }
    if settings.tray_style != old.tray_style {
        *state.last_look.lock().unwrap() = None; // force a redraw
    }
    core.mark_changed();
    let view = settings_view(&app, core);
    let _ = app.emit("settings-changed", &view);
    Ok(view)
}

#[tauri::command]
fn set_popup_height(window: WebviewWindow, state: tauri::State<AppState>, height: f64) {
    let h = height.clamp(120.0, 640.0);
    *state.popup_height.lock().unwrap() = h;
    let scale = window.scale_factor().unwrap_or(1.0);
    let new_w = (tray::popup_width() * scale).round() as u32;
    let new_h = (h * scale).round() as u32;
    let _ = window.set_size(tauri::PhysicalSize::new(new_w, new_h));
    // Re-pin to the anchor chosen when the popup was shown. The target position depends
    // only on the anchor and the new height, so repeated or racing calls cannot make
    // the popup drift (a relative "move by the difference" did exactly that).
    if let Some(anchor) = *state.anchor.lock().unwrap() {
        let (x, y) = anchor.window_pos(new_h as i32);
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
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

/// Asks the page to render fresh data and report its height first, so the window
/// appears once, at its final size and place: resizing a window that is already on
/// screen makes the system shadow jump around it. If the page doesn't answer in time
/// (it should take a few ms), the window is shown anyway.
fn show_popup<R: Runtime>(app: &AppHandle<R>, rect: Option<Rect>) {
    let state = app.state::<AppState>();
    let id = state.show_seq.fetch_add(1, Ordering::Relaxed) + 1;
    *state.pending_show.lock().unwrap() = Some((id, rect));
    let _ = app.emit("popup-prepare", id);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = reveal(&app, id, None);
    });
}

/// Shows the popup for show request `id` (once; later calls for the same id do nothing).
/// Returns whether this call showed it.
fn reveal<R: Runtime>(app: &AppHandle<R>, id: u64, height: Option<f64>) -> bool {
    let state = app.state::<AppState>();
    let rect = {
        let mut pending = state.pending_show.lock().unwrap();
        match *pending {
            Some((pid, rect)) if pid == id => {
                *pending = None;
                rect
            }
            _ => return false,
        }
    };
    let Some(win) = tray::popup(app) else { return false };
    let h = {
        let mut ph = state.popup_height.lock().unwrap();
        if let Some(h) = height {
            *ph = h.clamp(120.0, 640.0);
        }
        *ph
    };
    // Position first (computed for the monitor that holds the tray icon), then a
    // logical size, so the size follows that monitor's scale and not the one the
    // hidden window was last on.
    let anchor = tray::place_popup(&win, rect, h);
    *state.anchor.lock().unwrap() = anchor;
    let _ = win.set_size(tauri::LogicalSize::new(tray::popup_width(), h));
    let _ = win.show();
    let _ = win.set_focus();
    true
}

/// The page has rendered for show request `id` and measured itself. Returns false when
/// the request was already served (the fallback timer showed the window first): the
/// page then sends its height the ordinary way.
#[tauri::command]
fn reveal_popup(app: AppHandle, id: u64, height: f64) -> bool {
    reveal(&app, id, Some(height))
}

/// Forgets the saved EcoFlow password (signs out of the cloud).
#[tauri::command]
fn sign_out(app: AppHandle, state: tauri::State<AppState>) -> SettingsView {
    let core = &state.core;
    let email = core.settings().cloud_email;
    settings::delete_password(&email);
    core.set_has_password(&email, false);
    core.cloud_restart.notify_one();
    core.mark_changed();
    let view = settings_view(&app, core);
    let _ = app.emit("settings-changed", &view);
    view
}

fn toggle_popup<R: Runtime>(app: &AppHandle<R>, rect: Rect) {
    let Some(win) = tray::popup(app) else { return };
    let state = app.state::<AppState>();
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }
    // A second click while the first one is still being prepared: nothing to do.
    if state.pending_show.lock().unwrap().is_some() {
        return;
    }
    // The click that took focus away already hid it: treat as "close".
    if now_ms().saturating_sub(state.last_blur_hide.load(Ordering::Relaxed)) < 350 {
        return;
    }
    show_popup(app, Some(rect));
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
    let settings = state.core.settings();
    let scale = app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(1.0);
    let look = tray::look(snap, settings.low_threshold, tray::tray_px(scale), &settings.tray_style);
    // The unit sits next to the number on macOS, so a language change redraws it too.
    let unit = if snap.lang == "en" { "V" } else { "B" };
    let mut last = state.last_look.lock().unwrap();
    let drawn = (look, unit);
    if last.as_ref() != Some(&drawn) {
        let (look, unit) = &drawn;
        if cfg!(target_os = "macos") {
            // The number is part of the image (like the system battery item); a separate
            // title would use the large menu bar font.
            let icon = match look.kind {
                tray::Kind::Voltage => tray::render_voltage_template(&look.text, unit),
                tray::Kind::Percent => tray::render_battery_template(Some(&look.text), look.fill, look.charging),
            };
            let _ = tray_icon.set_icon(Some(icon));
            let _ = tray_icon.set_icon_as_template(true);
        } else if settings.tray_style == "battery" && look.kind == tray::Kind::Percent {
            let _ = tray_icon.set_icon(Some(tray::render_battery_color(look)));
        } else {
            let _ = tray_icon.set_icon(Some(tray::render_number(look)));
        }
        *last = Some(drawn);
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
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // `station-battery --settings` opens the settings window (e.g. from a shortcut).
            if args.iter().any(|a| a == "--settings") {
                open_settings_window(app);
            } else {
                show_popup(app, None);
            }
        }))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_platform,
            open_settings,
            reveal_window,
            set_window_height,
            get_snapshot,
            get_settings,
            save_settings,
            sign_out,
            reveal_popup,
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
                anchor: Mutex::new(None),
                popup_height: Mutex::new(236.0),
                pending_show: Mutex::new(None),
                show_seq: AtomicU64::new(0),
            });

            let snap = core.snapshot();
            let scale = app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(1.0);
            let look = tray::look(&snap, core.settings().low_threshold, tray::tray_px(scale), "number");
            let icon = if cfg!(target_os = "macos") {
                tray::render_battery_template(None, None, false)
            } else if core.settings().tray_style == "battery" {
                tray::render_battery_color(&look)
            } else {
                tray::render_number(&look)
            };
            let mut builder = TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .tooltip(tray::tooltip(&snap, &lang))
                .menu(&build_menu(app.handle(), &lang)?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_popup(app, None),
                    "settings" => open_settings_window(app),
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
                builder = builder.icon_as_template(true);
            }
            builder.build(app)?;

            if let Some(win) = tray::popup(app.handle()) {
                platform::apply_popup_effects(&win, &platform::info());
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

            if std::env::args().any(|a| a == "--settings") {
                open_settings_window(app.handle());
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
