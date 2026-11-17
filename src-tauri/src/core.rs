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
/// The connection counts as lost when the station's adverts stop for this long. It
/// advertises every second or two, so this is a long silence, not a missed packet.
pub const BLE_LIVE_MS: u64 = 45 * 1000;
/// Cloud data counts as live while the cloud reports the station online and the last
/// update is this recent (the app polls every minute).
pub const CLOUD_LIVE_MS: u64 = 3 * 60 * 1000;
/// The last known level is still shown (as "no connection") for this long.
pub const KEEP_MS: u64 = 12 * 60 * 60 * 1000;

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
    /// (email, has a stored password). Reading the system keychain can show a prompt on
    /// macOS, so the answer is remembered instead of being looked up for every UI refresh.
    pub password_known: Mutex<Option<(String, bool)>>,
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
    /// Grid (AC input) and AC output voltage, volts.
    pub volts_in: Option<i64>,
    pub volts_out: Option<i64>,
    /// Battery state of health, percent, and charge cycles.
    pub soh: Option<u8>,
    pub cycles: Option<u32>,
    pub rssi: Option<i16>,
    pub updated_at: u64,
    pub stale: bool,
    pub ble_status: String,
    pub cloud_status: String,
    pub cloud_message: String,
    pub devices: Vec<DeviceItem>,
    pub capacity_wh: Option<u32>,
    pub low_threshold: u8,
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
    pub volts_in: Option<i64>,
    pub volts_out: Option<i64>,
    pub soh: Option<u8>,
    pub cycles: Option<u32>,
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
    // The inverter reports millivolts; 0 means no grid (input) or AC switched off (output).
    // A value under 1000 cannot be a real mains reading in millivolts, so it is taken
    // as volts already (seen on other models) rather than turned into a false "0 V".
    let volts = |key: &str| {
        num(p, &[key]).and_then(|v| match v {
            v if v == 0.0 => Some(0),
            v if (1.0..1000.0).contains(&v) => Some(v.round() as i64).filter(|v| *v <= 400),
            v if (1000.0..=400_000.0).contains(&v) => Some((v / 1000.0).round() as i64),
            _ => None,
        })
    };
    let volts_in = volts("inv.acInVol");
    let volts_out = volts("inv.invOutVol");
    let soh = num(p, &["bms_bmsStatus.soh"]).filter(|v| (1.0..=100.0).contains(v)).map(|v| v.round() as u8);
    let cycles = num(p, &["bms_bmsStatus.cycles"]).filter(|v| (0.0..=100_000.0).contains(v)).map(|v| v.round() as u32);
    CloudValues { soc, watts_in, watts_out, minutes, charging, volts_in, volts_out, soh, cycles }
}

impl Core {
    pub fn new(settings: Settings, settings_path: PathBuf) -> Shared {
        Arc::new(Core {
            settings: RwLock::new(settings),
            settings_path,
            ble: Mutex::new(BleState { status: "starting".into(), ..Default::default() }),
            cloud: Mutex::new(CloudState { status: "off".into(), ..Default::default() }),
            notify: Mutex::new(NotifyState::default()),
            password_known: Mutex::new(None),
            ble_restart: Notify::new(),
            cloud_restart: Notify::new(),
            changed: Notify::new(),
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn has_password(&self, email: &str) -> bool {
        if email.is_empty() {
            return false;
        }
        if let Some((e, has)) = self.password_known.lock().unwrap().as_ref() {
            if e == email {
                return *has;
            }
        }
        let has = crate::settings::get_password(email).is_some();
        self.set_has_password(email, has);
        has
    }

    pub fn set_has_password(&self, email: &str, has: bool) {
        *self.password_known.lock().unwrap() = Some((email.to_string(), has));
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
            low_threshold: settings.low_threshold,
            sn: sn.clone(),
            ..Default::default()
        };

        // A reading is (level, time it was received, live?). "Live" means the station is
        // reachable right now; otherwise the value is the last one known.
        let age = |t: u64| now.saturating_sub(t);
        let mut ble_reading: Option<(u8, u64, bool)> = None;
        {
            let ble = self.ble.lock().unwrap();
            snap.ble_status = if settings.ble_enabled { ble.status.clone() } else { "off".into() };
            let mut list: Vec<DeviceItem> = ble
                .devices
                .iter()
                .filter(|(_, d)| age(d.last) < BLE_FRESH_MS)
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
                    if age(d.last) < BLE_LIVE_MS {
                        snap.rssi = d.rssi;
                    }
                    if let Some(soc) = d.soc {
                        if age(d.soc_at) < KEEP_MS {
                            ble_reading = Some((soc, d.soc_at, age(d.soc_at) < BLE_LIVE_MS));
                        }
                    }
                }
            }
        }

        let mut cloud_reading: Option<(u8, u64, bool)> = None;
        {
            let cloud = self.cloud.lock().unwrap();
            snap.cloud_status = if settings.cloud_enabled { cloud.status.clone() } else { "off".into() };
            snap.cloud_message = cloud.message.clone();
            if settings.cloud_enabled && cloud.sn == sn && !sn.is_empty() && cloud.last > 0 {
                let v = cloud_values(&cloud.params);
                let live = cloud.status == "online" && age(cloud.last) < CLOUD_LIVE_MS;
                if live {
                    // Power, voltage and time only mean something while they are current.
                    snap.watts_in = v.watts_in;
                    snap.watts_out = v.watts_out;
                    snap.minutes = v.minutes;
                    snap.charging = v.charging;
                    snap.volts_in = v.volts_in;
                    snap.volts_out = v.volts_out;
                }
                // Slow-moving facts about the battery stay valid without live data.
                snap.soh = v.soh;
                snap.cycles = v.cycles;
                if let Some(soc) = v.soc.filter(|_| age(cloud.last) < KEEP_MS) {
                    cloud_reading = Some((soc, cloud.last, live));
                }
            }
        }

        // A live reading beats a stale one; between two of the same kind, the newer wins.
        let pick = match (ble_reading, cloud_reading) {
            (Some(b), Some(c)) => {
                if (c.2, c.1) > (b.2, b.1) {
                    Some((c, "cloud"))
                } else {
                    Some((b, "ble"))
                }
            }
            (Some(b), None) => Some((b, "ble")),
            (None, Some(c)) => Some((c, "cloud")),
            (None, None) => None,
        };
        if let Some(((soc, at, live), source)) = pick {
            snap.soc = Some(soc);
            snap.source = source.into();
            snap.updated_at = at;
            snap.stale = !live;
        }
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
        p.insert("inv.acInVol".into(), json!(228400));
        p.insert("inv.invOutVol".into(), json!(0));
        p.insert("bms_bmsStatus.soh".into(), json!(98));
        p.insert("bms_bmsStatus.cycles".into(), json!(42));
        let v = cloud_values(&p);
        assert_eq!(v.soc, Some(78));
        assert_eq!(v.charging, Some(true));
        assert_eq!(v.minutes, Some(25));
        assert_eq!((v.volts_in, v.volts_out), (Some(228), Some(0)));
        assert_eq!((v.soh, v.cycles), (Some(98), Some(42)));
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
        // Not reported (or garbage) stays unknown rather than becoming zero.
        p.insert("inv.acInVol".into(), json!(-5));
        p.insert("bms_bmsStatus.soh".into(), json!(0));
        let v = cloud_values(&p);
        assert_eq!((v.volts_in, v.volts_out, v.soh, v.cycles), (None, None, None, None));
        // Already in volts: kept as is, not divided down to zero.
        p.insert("inv.acInVol".into(), json!(231));
        p.insert("inv.invOutVol".into(), json!(229900));
        let v = cloud_values(&p);
        assert_eq!((v.volts_in, v.volts_out), (Some(231), Some(230)));
        p.insert("inv.acInVol".into(), json!(999_000_000));
        assert_eq!(cloud_values(&p).volts_in, None);
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
    fn ble_link_is_lost_after_silence_but_the_last_level_stays() {
        let c = core();
        see(&c, "R621AAAAAAAAAAAA", -60, Some(78), 1000);
        let s = c.snapshot();
        assert_eq!((s.soc, s.source.as_str(), s.stale, s.rssi), (Some(78), "ble", false, Some(-60)));
        // Station switched off: 45 s without adverts = no connection, last level kept.
        see(&c, "R621AAAAAAAAAAAA", -60, Some(78), BLE_LIVE_MS + 1000);
        let s = c.snapshot();
        assert_eq!((s.soc, s.stale, s.rssi), (Some(78), true, None));
        // Much later it is forgotten.
        see(&c, "R621AAAAAAAAAAAA", -60, Some(78), KEEP_MS + 1000);
        assert_eq!(c.snapshot().soc, None);
        let _ = std::fs::remove_file(&c.settings_path);
    }

    fn with_cloud(c: &Shared, status: &str, age_ms: u64, soc: u8) {
        let mut s = c.settings.write().unwrap();
        s.cloud_enabled = true;
        s.device_sn = "R621AAAAAAAAAAAA".into();
        drop(s);
        let mut cl = c.cloud.lock().unwrap();
        cl.sn = "R621AAAAAAAAAAAA".into();
        cl.status = status.into();
        cl.last = now_ms() - age_ms;
        cl.params.insert("pd.soc".into(), serde_json::json!(soc));
        cl.params.insert("pd.wattsInSum".into(), serde_json::json!(300));
        cl.params.insert("pd.wattsOutSum".into(), serde_json::json!(20));
        cl.params.insert("inv.acInVol".into(), serde_json::json!(230000));
    }

    #[test]
    fn cloud_offline_is_no_connection() {
        let c = core();
        with_cloud(&c, "online", 5_000, 64);
        let s = c.snapshot();
        assert_eq!((s.soc, s.source.as_str(), s.stale, s.watts_in, s.volts_in), (Some(64), "cloud", false, Some(300), Some(230)));
        // The cloud says the station went offline: no connection, no live power data.
        with_cloud(&c, "offline", 5_000, 64);
        let s = c.snapshot();
        assert_eq!((s.soc, s.stale, s.watts_in, s.volts_in, s.charging), (Some(64), true, None, None, None));
        // Online but silent for too long: also lost.
        with_cloud(&c, "online", CLOUD_LIVE_MS + 1000, 64);
        assert!(c.snapshot().stale);
        let _ = std::fs::remove_file(&c.settings_path);
    }

    #[test]
    fn a_live_source_beats_a_newer_stale_one() {
        let c = core();
        // Cloud offline but its last value is newer than the live Bluetooth one.
        with_cloud(&c, "offline", 1_000, 50);
        see(&c, "R621AAAAAAAAAAAA", -60, Some(77), 3_000);
        let s = c.snapshot();
        assert_eq!((s.soc, s.source.as_str(), s.stale), (Some(77), "ble", false));
        let _ = std::fs::remove_file(&c.settings_path);
    }
}
