use crate::models;
use crate::settings::Settings;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

/// A BLE reading is used for this long after the last advert.
pub const BLE_FRESH_MS: u64 = 3 * 60 * 1000;
/// The popup marks the value as outdated after this.
pub const STALE_MS: u64 = 5 * 60 * 1000;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
pub struct BleSeen {
    pub soc: Option<u8>,
    pub rssi: Option<i16>,
    pub last: u64,
    /// Last time `soc` was received (adverts without soc don't refresh it).
    pub soc_at: u64,
}

#[derive(Default)]
pub struct BleState {
    /// "off" | "starting" | "scanning" | "no_adapter" | "error"
    pub status: String,
    pub devices: HashMap<String, BleSeen>,
}

#[derive(Default)]
pub struct CloudState {
    /// "off" | "no_login" | "no_device" | "connecting" | "online" | "offline" | "auth_error" | "error"
    pub status: String,
    pub message: String,
    pub sn: String,
    pub params: HashMap<String, Value>,
    pub last: u64,
}

#[derive(Default)]
pub struct NotifyState {
    pub low_armed: bool,
    pub full_armed: bool,
    pub initialized: bool,
    pub sn: String,
}

pub struct Core {
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
    pub ble: Mutex<BleState>,
    pub cloud: Mutex<CloudState>,
    pub notify: Mutex<NotifyState>,
    pub ble_restart: Notify,
    pub cloud_restart: Notify,
    pub changed: Notify,
}

pub type Shared = Arc<Core>;

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceItem {
    pub sn: String,
    pub model: String,
    pub soc: Option<u8>,
    pub rssi: Option<i16>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub lang: String,
    pub sn: String,
    pub model: String,
    pub soc: Option<u8>,
    /// "ble" | "cloud" | ""
    pub source: String,
    pub watts_in: Option<i64>,
    pub watts_out: Option<i64>,
    /// Minutes to full (when charging) or to empty.
    pub minutes: Option<i64>,
    pub charging: Option<bool>,
    pub rssi: Option<i16>,
    pub updated_at: u64,
    pub stale: bool,
    pub ble_status: String,
    pub cloud_status: String,
    pub cloud_message: String,
    pub devices: Vec<DeviceItem>,
    pub capacity_wh: Option<u32>,
}

fn num(params: &HashMap<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| params.get(*k).and_then(|v| v.as_f64()))
}

pub fn capacity_wh(sn: &str) -> Option<u32> {
    match models::name(sn).as_str() {
        "River 2" => Some(256),
        "River 2 Max" => Some(512),
        "River 2 Pro" => Some(768),
        "Delta 2" => Some(1024),
        "Delta 2 Max" => Some(2048),
        "River 3" => Some(245),
        _ => None,
    }
}

/// Values derived from the cloud parameter map (JSON-protocol devices).
#[derive(Debug, Default, PartialEq)]
pub struct CloudValues {
    pub soc: Option<u8>,
    pub watts_in: Option<i64>,
    pub watts_out: Option<i64>,
    pub minutes: Option<i64>,
    pub charging: Option<bool>,
}

pub fn cloud_values(p: &HashMap<String, Value>) -> CloudValues {
    let soc = num(p, &["pd.soc", "bms_bmsStatus.soc", "bmsMaster.soc", "bms_bmsStatus.f32ShowSoc"])
        .filter(|v| (0.0..=100.0).contains(v))
        .map(|v| v.round() as u8);
    let watts_in = num(p, &["pd.wattsInSum", "inv.inputWatts"]).map(|v| v.round() as i64);
    let watts_out = num(p, &["pd.wattsOutSum", "inv.outputWatts"]).map(|v| v.round() as i64);
    let charging = match (watts_in, watts_out) {
        (Some(i), Some(o)) => Some(i > o && i > 0),
        (Some(i), None) => Some(i > 0),
        _ => None,
    };
    let valid = |m: f64| m > 0.0 && m < 5999.0;
    let minutes = if charging == Some(true) {
        num(p, &["bms_emsStatus.chgRemainTime", "ems.chgRemainTime"])
            .filter(|m| valid(*m))
            .or_else(|| num(p, &["pd.remainTime"]).map(f64::abs).filter(|m| valid(*m)))
    } else {
        num(p, &["bms_emsStatus.dsgRemainTime", "ems.dsgRemainTime"])
            .filter(|m| valid(*m))
            .or_else(|| num(p, &["pd.remainTime"]).map(f64::abs).filter(|m| valid(*m)))
    }
    .map(|m| m.round() as i64);
    CloudValues { soc, watts_in, watts_out, minutes, charging }
}

impl Core {
    pub fn new(settings: Settings, settings_path: PathBuf) -> Shared {
        Arc::new(Core {
            settings: RwLock::new(settings),
            settings_path,
            ble: Mutex::new(BleState { status: "starting".into(), ..Default::default() }),
            cloud: Mutex::new(CloudState { status: "off".into(), ..Default::default() }),
            notify: Mutex::new(NotifyState::default()),
            ble_restart: Notify::new(),
            cloud_restart: Notify::new(),
            changed: Notify::new(),
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn mark_changed(&self) {
        self.changed.notify_one();
    }

    /// Serial used for display: the configured one, else the nearest BLE station
    /// whose percentage we can read.
    pub fn effective_sn(&self) -> String {
        let s = self.settings();
        if !s.device_sn.is_empty() {
            return s.device_sn;
        }
        let now = now_ms();
        let nearest = {
            let ble = self.ble.lock().unwrap();
            let fresh = |d: &BleSeen| now.saturating_sub(d.last) < BLE_FRESH_MS;
            // Stay with the station already chosen while it is still in range, so two
            // stations with similar signal don't keep swapping places.
            if ble.devices.get(&s.last_seen_sn).is_some_and(fresh) {
                return s.last_seen_sn;
            }
            ble.devices
                .iter()
                .filter(|(_, d)| fresh(d))
                .max_by_key(|(sn, d)| (d.soc.is_some(), d.rssi.unwrap_or(-200), (*sn).clone()))
                .map(|(sn, _)| sn.clone())
        };
        match nearest {
            Some(sn) => {
                if sn != s.last_seen_sn {
                    self.remember_sn(&sn);
                }
                sn
            }
            None => s.last_seen_sn,
        }
    }

    fn remember_sn(&self, sn: &str) {
        let mut w = self.settings.write().unwrap();
        // Another thread may have stored it meanwhile, or the user picked a station.
        if w.last_seen_sn == sn || !w.device_sn.is_empty() {
            return;
        }
        w.last_seen_sn = sn.to_string();
        let copy = w.clone();
        drop(w);
        if let Err(e) = crate::settings::save(&self.settings_path, &copy) {
            log::warn!("save settings: {e}");
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let settings = self.settings();
        let sn = self.effective_sn();
        let now = now_ms();
        let mut snap = Snapshot {
            lang: settings.resolved_lang().to_string(),
            model: if sn.is_empty() { String::new() } else { models::name(&sn) },
            capacity_wh: capacity_wh(&sn),
            sn: sn.clone(),
            ..Default::default()
        };

        let mut ble_reading: Option<(u8, u64)> = None;
        {
            let ble = self.ble.lock().unwrap();
            snap.ble_status = if settings.ble_enabled { ble.status.clone() } else { "off".into() };
            let mut list: Vec<DeviceItem> = ble
                .devices
                .iter()
                .filter(|(_, d)| now.saturating_sub(d.last) < BLE_FRESH_MS)
                .map(|(sn, d)| DeviceItem {
                    sn: sn.clone(),
                    model: models::name(sn),
                    soc: d.soc,
                    rssi: d.rssi,
                })
                .collect();
            list.sort_by(|a, b| b.rssi.unwrap_or(-200).cmp(&a.rssi.unwrap_or(-200)).then(a.sn.cmp(&b.sn)));
            snap.devices = list;
            if settings.ble_enabled {
                if let Some(d) = ble.devices.get(&sn) {
                    if now.saturating_sub(d.last) < BLE_FRESH_MS {
                        snap.rssi = d.rssi;
                    }
                    if let Some(soc) = d.soc {
                        if now.saturating_sub(d.soc_at) < BLE_FRESH_MS {
                            ble_reading = Some((soc, d.soc_at));
                        }
                    }
                }
            }
        }

        {
            let cloud = self.cloud.lock().unwrap();
            snap.cloud_status = if settings.cloud_enabled { cloud.status.clone() } else { "off".into() };
            snap.cloud_message = cloud.message.clone();
            if settings.cloud_enabled && cloud.sn == sn && !sn.is_empty() && cloud.last > 0 {
                let v = cloud_values(&cloud.params);
                let cloud_fresh = now.saturating_sub(cloud.last) < STALE_MS * 2;
                if cloud_fresh {
                    snap.watts_in = v.watts_in;
                    snap.watts_out = v.watts_out;
                    snap.minutes = v.minutes;
                    snap.charging = v.charging;
                }
                if let Some(soc) = v.soc {
                    let newer_than_ble = ble_reading.map(|(_, t)| cloud.last > t).unwrap_or(true);
                    if newer_than_ble {
                        snap.soc = Some(soc);
                        snap.source = "cloud".into();
                        snap.updated_at = cloud.last;
                    }
                }
            }
        }

        if snap.source.is_empty() {
            if let Some((soc, t)) = ble_reading {
                snap.soc = Some(soc);
                snap.source = "ble".into();
                snap.updated_at = t;
            }
        }
        snap.stale = snap.soc.is_some() && now.saturating_sub(snap.updated_at) > STALE_MS;
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cloud_river2_values() {
        let mut p = HashMap::new();
        p.insert("pd.soc".into(), json!(78));
        p.insert("pd.wattsInSum".into(), json!(342));
        p.insert("pd.wattsOutSum".into(), json!(45));
        p.insert("bms_emsStatus.chgRemainTime".into(), json!(25));
        p.insert("bms_emsStatus.dsgRemainTime".into(), json!(5999));
        let v = cloud_values(&p);
        assert_eq!(v.soc, Some(78));
        assert_eq!(v.charging, Some(true));
        assert_eq!(v.minutes, Some(25));
    }

    #[test]
    fn cloud_discharging_ignores_placeholder() {
        let mut p = HashMap::new();
        p.insert("bms_bmsStatus.soc".into(), json!(40));
        p.insert("pd.wattsInSum".into(), json!(0));
        p.insert("pd.wattsOutSum".into(), json!(60));
        p.insert("bms_emsStatus.dsgRemainTime".into(), json!(5999));
        let v = cloud_values(&p);
        assert_eq!(v.soc, Some(40));
        assert_eq!(v.charging, Some(false));
        assert_eq!(v.minutes, None);
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    fn core() -> Shared {
        let path = std::env::temp_dir().join(format!("sb-sel-{}.json", now_ms()));
        Core::new(Settings::default(), path)
    }

    fn see(core: &Shared, sn: &str, rssi: i16, soc: Option<u8>, age_ms: u64) {
        let t = now_ms() - age_ms;
        core.ble.lock().unwrap().devices.insert(sn.into(), BleSeen { soc, rssi: Some(rssi), last: t, soc_at: t });
    }

    #[test]
    fn auto_pick_is_sticky_and_survives_out_of_range() {
        let c = core();
        assert_eq!(c.effective_sn(), "");
        see(&c, "R621AAAAAAAAAAAA", -70, Some(50), 0);
        assert_eq!(c.effective_sn(), "R621AAAAAAAAAAAA");
        // A stronger neighbour appears: keep the one already chosen.
        see(&c, "R601BBBBBBBBBBBB", -40, Some(90), 0);
        assert_eq!(c.effective_sn(), "R621AAAAAAAAAAAA");
        // The chosen one leaves range: switch to what is still visible.
        see(&c, "R621AAAAAAAAAAAA", -70, Some(50), BLE_FRESH_MS + 1000);
        assert_eq!(c.effective_sn(), "R601BBBBBBBBBBBB");
        // Nothing in range: remember the last one (keeps the cloud session alive).
        see(&c, "R601BBBBBBBBBBBB", -40, Some(90), BLE_FRESH_MS + 1000);
        assert_eq!(c.effective_sn(), "R601BBBBBBBBBBBB");
        let _ = std::fs::remove_file(&c.settings_path);
    }

    #[test]
    fn explicit_choice_wins() {
        let c = core();
        c.settings.write().unwrap().device_sn = "R351CCCCCCCCCCCC".into();
        see(&c, "R621AAAAAAAAAAAA", -40, Some(50), 0);
        assert_eq!(c.effective_sn(), "R351CCCCCCCCCCCC");
        let snap = c.snapshot();
        assert_eq!(snap.model, "Delta 2 Max");
        assert_eq!(snap.soc, None); // the selected station is not the one in range
    }

    #[test]
    fn ble_reading_expires() {
        let c = core();
        see(&c, "R621AAAAAAAAAAAA", -60, Some(78), 1000);
        let s = c.snapshot();
        assert_eq!((s.soc, s.source.as_str()), (Some(78), "ble"));
        see(&c, "R621AAAAAAAAAAAA", -60, Some(78), BLE_FRESH_MS + 1000);
        assert_eq!(c.snapshot().soc, None);
        let _ = std::fs::remove_file(&c.settings_path);
    }
}
