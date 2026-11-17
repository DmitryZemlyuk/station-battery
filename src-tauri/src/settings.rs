use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "io.github.dmitryzemlyuk.stationbattery";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// "auto" | "uk" | "ru" | "en"
    pub lang: String,
    pub ble_enabled: bool,
    pub cloud_enabled: bool,
    pub cloud_email: String,
    /// "eu" | "global" | "america"
    pub cloud_region: String,
    /// Selected station serial; empty = nearest one seen over Bluetooth.
    pub device_sn: String,
    pub notify_low: bool,
    pub low_threshold: u8,
    pub notify_full: bool,
    pub autostart: bool,
    /// Tray / menu bar item: "number" (percentage) | "battery" (glyph, Windows only)
    /// | "voltage" (grid voltage from the cloud; falls back to the percentage)
    pub tray_style: String,
    /// Size of the digits in the tray / menu bar: "s" | "m" (like the system text) | "l" | "xl"
    pub tray_digit_size: String,
    /// Last station picked automatically (keeps the cloud working when it leaves BLE range).
    pub last_seen_sn: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lang: "auto".into(),
            ble_enabled: true,
            cloud_enabled: false,
            cloud_email: String::new(),
            cloud_region: "eu".into(),
            device_sn: String::new(),
            notify_low: true,
            low_threshold: 20,
            notify_full: true,
            autostart: false,
            tray_style: "number".into(),
            tray_digit_size: "m".into(),
            last_seen_sn: String::new(),
        }
    }
}

impl Settings {
    pub fn sanitize(&mut self) {
        if !["auto", "uk", "ru", "en"].contains(&self.lang.as_str()) {
            self.lang = "auto".into();
        }
        if !["eu", "global", "america"].contains(&self.cloud_region.as_str()) {
            self.cloud_region = "eu".into();
        }
        if !["number", "battery", "voltage"].contains(&self.tray_style.as_str()) {
            self.tray_style = "number".into();
        }
        if !["s", "m", "l", "xl"].contains(&self.tray_digit_size.as_str()) {
            self.tray_digit_size = "m".into();
        }
        self.low_threshold = self.low_threshold.clamp(5, 90);
        self.cloud_email = self.cloud_email.trim().to_string();
        self.device_sn = self.device_sn.trim().to_uppercase();
    }

    pub fn api_host(&self) -> &'static str {
        match self.cloud_region.as_str() {
            "global" => "api.ecoflow.com",
            "america" => "api-a.ecoflow.com",
            _ => "api-e.ecoflow.com",
        }
    }

    /// Resolved UI language.
    pub fn resolved_lang(&self) -> &'static str {
        match self.lang.as_str() {
            "uk" => "uk",
            "ru" => "ru",
            "en" => "en",
            _ => {
                let loc = sys_locale::get_locale().unwrap_or_default().to_lowercase();
                if loc.starts_with("uk") {
                    "uk"
                } else if loc.starts_with("ru") {
                    "ru"
                } else {
                    "en"
                }
            }
        }
    }
}

pub fn load(path: &PathBuf) -> Settings {
    let mut s = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
        .unwrap_or_default();
    s.sanitize();
    s
}

pub fn save(path: &PathBuf, s: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn entry(email: &str) -> Option<keyring::Entry> {
    if email.is_empty() {
        return None;
    }
    keyring::Entry::new(KEYRING_SERVICE, email).ok()
}

pub fn get_password(email: &str) -> Option<String> {
    entry(email)?.get_password().ok().filter(|p| !p.is_empty())
}

pub fn set_password(email: &str, password: &str) -> Result<(), String> {
    let e = entry(email).ok_or("empty email")?;
    e.set_password(password).map_err(|e| e.to_string())
}

pub fn delete_password(email: &str) {
    if let Some(e) = entry(email) {
        let _ = e.delete_credential();
    }
}
