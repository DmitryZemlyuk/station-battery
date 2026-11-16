//! Platform look: system accent colour, backdrop materials, the settings window.

use serde::Serialize;
use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    /// "windows" | "macos" | "linux"
    pub os: String,
    /// The window has a system material behind it (Acrylic / Mica / vibrancy):
    /// the page must keep its background transparent.
    pub backdrop: bool,
    /// Windows accent shades (`#rrggbb`): Fluent uses "Dark 1" on light theme and
    /// "Light 2" on dark theme. Empty elsewhere (macOS uses the CSS system colour).
    pub accent_on_light: String,
    pub accent_on_dark: String,
}

pub fn info() -> PlatformInfo {
    let os = std::env::consts::OS.to_string();
    #[cfg(windows)]
    {
        let (accent_on_light, accent_on_dark) = windows_accent();
        return PlatformInfo { os, backdrop: windows_has_backdrop(), accent_on_light, accent_on_dark };
    }
    #[cfg(target_os = "macos")]
    {
        return PlatformInfo { os, backdrop: true, accent_on_light: String::new(), accent_on_dark: String::new() };
    }
    #[allow(unreachable_code)]
    PlatformInfo { os, backdrop: false, accent_on_light: String::new(), accent_on_dark: String::new() }
}

/// Windows 11 22H2+ (build 22621) has the system backdrop types (Mica, transient
/// Acrylic). Older builds get an opaque fallback instead of the laggy legacy acrylic.
#[cfg(windows)]
pub fn windows_has_backdrop() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
        .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
        .ok()
        .and_then(|b| b.parse::<u32>().ok())
        .is_some_and(|b| b >= 22621)
}

/// Reads the accent palette Windows generates from the user's accent colour.
/// `AccentPalette` is 8 RGBA entries: Light3, Light2, Light1, Base, Dark1, Dark2, Dark3, …
#[cfg(windows)]
fn windows_accent() -> (String, String) {
    use winreg::enums::HKEY_CURRENT_USER;
    let fallback = ("#005FB8".to_string(), "#60CDFF".to_string());
    let Ok(key) = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent")
    else {
        return fallback;
    };
    let Ok(raw) = key.get_raw_value("AccentPalette") else { return fallback };
    palette_shades(&raw.bytes).unwrap_or(fallback)
}

/// (Dark 1, Light 2) from the raw palette bytes.
#[cfg(any(windows, test))]
pub fn palette_shades(bytes: &[u8]) -> Option<(String, String)> {
    if bytes.len() < 32 {
        return None;
    }
    let hex = |i: usize| format!("#{:02X}{:02X}{:02X}", bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2]);
    Some((hex(4), hex(1)))
}

/// Material behind the tray popup.
pub fn apply_popup_effects<R: Runtime>(win: &WebviewWindow<R>, info: &PlatformInfo) {
    if !info.backdrop {
        return;
    }
    if cfg!(target_os = "macos") {
        let _ = win.set_effects(
            EffectsBuilder::new().effect(Effect::Popover).state(EffectState::Active).radius(12.0).build(),
        );
        let _ = win.set_shadow(true);
    } else if cfg!(windows) {
        // Transient Acrylic is what the system flyouts (volume, battery) use.
        let _ = win.set_effects(EffectsBuilder::new().effect(Effect::Acrylic).build());
        // Undecorated + shadow gives the rounded corners and shadow of Windows 11.
        let _ = win.set_shadow(true);
    }
}

/// Opens (or focuses) the settings window.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>, title: &str) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let info = info();
    let mut b = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title(title)
        .visible(false)
        .focused(true);

    #[cfg(target_os = "macos")]
    {
        b = b
            .inner_size(500.0, 360.0)
            .resizable(false)
            .minimizable(false)
            .maximizable(false)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .transparent(true)
            .effects(EffectsBuilder::new().effect(Effect::WindowBackground).state(EffectState::FollowsWindowActiveState).build());
    }
    #[cfg(windows)]
    {
        b = b.inner_size(560.0, 640.0).min_inner_size(440.0, 420.0).maximizable(false);
        if info.backdrop {
            b = b.transparent(true).effects(EffectsBuilder::new().effect(Effect::Mica).build());
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        b = b.inner_size(560.0, 640.0);
    }
    let _ = info;

    match b.build() {
        Ok(w) => {
            let _ = w.center();
            // The page shows the window once it has laid itself out (no white flash).
        }
        Err(e) => {
            eprintln!("settings window: {e}");
            log::warn!("settings window: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_palette() {
        let mut b = vec![0u8; 32];
        // Light2 at index 1, Dark1 at index 4
        b[4..8].copy_from_slice(&[0x99, 0xEB, 0xFF, 0]);
        b[16..20].copy_from_slice(&[0x00, 0x5A, 0x9E, 0]);
        assert_eq!(palette_shades(&b), Some(("#005A9E".into(), "#99EBFF".into())));
        assert_eq!(palette_shades(&b[..10]), None);
    }
}
