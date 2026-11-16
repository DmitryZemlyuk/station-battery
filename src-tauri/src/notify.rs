//! Threshold notifications (low / full).

use crate::core::{Shared, Snapshot};
use crate::i18n;
use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;

pub enum Kind {
    Low(u8),
    Full,
}

/// Decide whether a notification is due. Pure logic, kept separate for tests.
pub fn evaluate(core: &Shared, snap: &Snapshot) -> Option<Kind> {
    let s = core.settings();
    let soc = snap.soc?;
    if snap.stale {
        return None;
    }
    let low = s.low_threshold;
    let mut n = core.notify.lock().unwrap();
    if !n.initialized || n.sn != snap.sn {
        n.initialized = true;
        n.sn = snap.sn.clone();
        n.low_armed = soc > low;
        n.full_armed = soc < 100;
        return None;
    }
    if soc > low.saturating_add(2) {
        n.low_armed = true;
    }
    if soc < 97 {
        n.full_armed = true;
    }
    if soc <= low && n.low_armed && snap.charging != Some(true) {
        n.low_armed = false;
        if s.notify_low {
            return Some(Kind::Low(soc));
        }
    }
    if soc >= 100 && n.full_armed {
        n.full_armed = false;
        if s.notify_full {
            return Some(Kind::Full);
        }
    }
    None
}

pub fn send<R: Runtime>(app: &AppHandle<R>, snap: &Snapshot, kind: Kind) {
    let t = i18n::get(&snap.lang);
    let name = if snap.model.is_empty() { "EcoFlow" } else { snap.model.as_str() };
    let (title, body) = match kind {
        Kind::Low(soc) => (t.low_title, t.low_body.replace("{name}", name).replace("{soc}", &soc.to_string())),
        Kind::Full => (t.full_title, t.full_body.replace("{name}", name)),
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        log::warn!("notification: {e}");
    }
}

/// Windows shows toasts from unpackaged apps only for a registered AppUserModelID.
/// Registering it under HKCU needs no admin rights.
#[cfg(windows)]
pub fn register_app_id(identifier: &str, display_name: &str) {
    use winreg::enums::HKEY_CURRENT_USER;
    let path = format!("Software\\Classes\\AppUserModelId\\{identifier}");
    if let Ok((key, _)) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path) {
        let _ = key.set_value("DisplayName", &display_name);
    }
}

#[cfg(not(windows))]
pub fn register_app_id(_identifier: &str, _display_name: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Core;
    use crate::settings::Settings;

    fn snap(soc: u8, charging: bool) -> Snapshot {
        Snapshot { soc: Some(soc), charging: Some(charging), sn: "R621".into(), ..Default::default() }
    }

    #[test]
    fn low_fires_once_with_hysteresis() {
        let core = Core::new(Settings::default(), std::env::temp_dir().join("sb-n.json"));
        assert!(evaluate(&core, &snap(30, false)).is_none()); // init
        assert!(evaluate(&core, &snap(21, false)).is_none());
        assert!(matches!(evaluate(&core, &snap(20, false)), Some(Kind::Low(20))));
        assert!(evaluate(&core, &snap(19, false)).is_none());
        assert!(evaluate(&core, &snap(21, true)).is_none()); // not re-armed yet
        assert!(evaluate(&core, &snap(23, true)).is_none()); // re-armed
        assert!(matches!(evaluate(&core, &snap(20, false)), Some(Kind::Low(20))));
    }

    #[test]
    fn full_fires_once() {
        let core = Core::new(Settings::default(), std::env::temp_dir().join("sb-n2.json"));
        assert!(evaluate(&core, &snap(95, true)).is_none());
        assert!(matches!(evaluate(&core, &snap(100, true)), Some(Kind::Full)));
        assert!(evaluate(&core, &snap(100, false)).is_none());
        assert!(evaluate(&core, &snap(98, false)).is_none());
        assert!(evaluate(&core, &snap(100, true)).is_none());
    }

    #[test]
    fn starting_at_full_does_not_fire() {
        let core = Core::new(Settings::default(), std::env::temp_dir().join("sb-n3.json"));
        assert!(evaluate(&core, &snap(100, false)).is_none());
        assert!(evaluate(&core, &snap(100, false)).is_none());
    }
}
