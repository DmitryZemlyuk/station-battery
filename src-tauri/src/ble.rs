//! Passive BLE scanner: reads the EcoFlow advertisement (no pairing, no connection).

use crate::core::{now_ms, BleSeen, Shared, BLE_FRESH_MS};
use crate::models::{parse_advert, MANUFACTURER_ID};
use btleplug::api::{Central, CentralEvent, CentralState, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, PeripheralId};
use futures::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

/// Rebuild the scan session after this long without any EcoFlow advert...
const WATCHDOG_MIN_MS: u64 = 3 * 60 * 1000;
/// ...backing off to this while no station is around. A rebuild is not free on
/// Windows (btleplug keeps a small per-adapter allocation alive through its radio
/// state handler), so an idle app must not rebuild every few minutes forever.
const WATCHDOG_MAX_MS: u64 = 30 * 60 * 1000;

enum End {
    /// Settings changed or the radio came back: start again right away.
    Requested,
    /// Nothing heard for the watchdog period.
    Watchdog,
}

/// macOS: every btleplug adapter owns a thread and a CBCentralManager that are never
/// released, so one adapter is created and reused for the life of the process (calling
/// start_scan on it again is harmless there).
/// Windows: the opposite. start_scan registers one more advert handler on the same
/// watcher each time, so every session gets a fresh adapter, which is cheap and is
/// released when dropped.
const REUSE_ADAPTER: bool = cfg!(target_os = "macos");

fn set_status(core: &Shared, status: &str) {
    let mut ble = core.ble.lock().unwrap();
    if ble.status != status {
        ble.status = status.to_string();
        drop(ble);
        core.mark_changed();
    }
}

pub async fn run(core: Shared) {
    let mut cached: Option<Adapter> = None;
    let mut watchdog_ms = WATCHDOG_MIN_MS;
    loop {
        if !core.settings().ble_enabled {
            set_status(&core, "off");
            core.ble_restart.notified().await;
            continue;
        }
        if core.ble.lock().unwrap().status == "off" {
            set_status(&core, "starting");
        }
        let heard_before = last_advert(&core);
        match scan_session(&core, &mut cached, watchdog_ms).await {
            Ok(end) => {
                let heard = last_advert(&core) > heard_before;
                watchdog_ms = match end {
                    End::Watchdog if !heard => (watchdog_ms * 2).min(WATCHDOG_MAX_MS),
                    _ => WATCHDOG_MIN_MS,
                };
            }
            Err(status) => {
                set_status(&core, status);
                tokio::select! {
                    _ = core.ble_restart.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(20)) => {}
                }
            }
        }
    }
}

fn last_advert(core: &Shared) -> u64 {
    core.ble.lock().unwrap().devices.values().map(|d| d.last).max().unwrap_or(0)
}

async fn adapter() -> Result<Adapter, &'static str> {
    let classify = |e: btleplug::Error| {
        log::warn!("BLE adapter: {e}");
        match e {
            btleplug::Error::PermissionDenied => "no_permission",
            _ => "no_adapter",
        }
    };
    let manager = Manager::new().await.map_err(classify)?;
    let adapters = manager.adapters().await.map_err(classify)?;
    adapters.into_iter().next().ok_or("no_adapter")
}

fn handle(core: &Shared, data: &[u8], rssi: Option<i16>) {
    let Some(adv) = parse_advert(data) else { return };
    let now = now_ms();
    let mut changed = false;
    {
        let mut ble = core.ble.lock().unwrap();
        let entry = ble.devices.entry(adv.serial.clone()).or_insert_with(|| {
            changed = true;
            BleSeen { soc: None, rssi: None, last: 0, soc_at: 0 }
        });
        if now.saturating_sub(entry.last) > BLE_FRESH_MS {
            changed = true; // came back after being gone
        }
        entry.last = now;
        if rssi.is_some() {
            entry.rssi = rssi;
        }
        if let Some(soc) = adv.soc {
            if entry.soc != Some(soc) || now.saturating_sub(entry.soc_at) > 60_000 {
                changed = true;
            }
            entry.soc = Some(soc);
            entry.soc_at = now;
        }
    }
    if changed {
        core.mark_changed();
    }
}

async fn read_props(central: &Adapter, id: &PeripheralId, core: &Shared) {
    let Ok(p) = central.peripheral(id).await else { return };
    let Ok(Some(props)) = p.properties().await else { return };
    if let Some(data) = props.manufacturer_data.get(&MANUFACTURER_ID) {
        handle(core, data, props.rssi);
    }
}

async fn scan_session(core: &Shared, cached: &mut Option<Adapter>, watchdog_ms: u64) -> Result<End, &'static str> {
    let central = match cached.as_ref() {
        Some(a) if REUSE_ADAPTER => a.clone(),
        _ => {
            let a = adapter().await?;
            if REUSE_ADAPTER {
                *cached = Some(a.clone());
            }
            a
        }
    };
    let mut events = central.events().await.map_err(|_| "error")?;

    // Bluetooth may be switched off, or (macOS, first launch) still waiting for the
    // permission prompt: the state then arrives later as a StateUpdate event.
    let mut powered = central.adapter_state().await.unwrap_or(CentralState::Unknown) != CentralState::PoweredOff;
    if powered {
        central.start_scan(ScanFilter::default()).await.map_err(|e| {
            log::warn!("BLE start_scan: {e}");
            "error"
        })?;
        set_status(core, "scanning");
    } else {
        set_status(core, "bt_off");
    }

    // Remember which peripherals are EcoFlow so RSSI updates are cheap to follow.
    let mut ecoflow: HashMap<PeripheralId, ()> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    let mut quiet_since = now_ms();
    let result = loop {
        tokio::select! {
            _ = core.ble_restart.notified() => break Ok(End::Requested),
            _ = tick.tick() => {
                // Lets the popup notice devices going away / values getting old.
                core.mark_changed();
                if !powered {
                    continue;
                }
                // Some stacks stop scanning silently (e.g. after sleep): when nothing
                // EcoFlow was heard for a while, rebuild the session.
                if now_ms().saturating_sub(last_advert(core).max(quiet_since)) > watchdog_ms {
                    break Ok(End::Watchdog);
                }
            }
            ev = events.next() => {
                let Some(ev) = ev else { break Err("error") };
                match ev {
                    CentralEvent::StateUpdate(CentralState::PoweredOff) => {
                        powered = false;
                        set_status(core, "bt_off");
                    }
                    CentralEvent::StateUpdate(CentralState::PoweredOn) => {
                        if REUSE_ADAPTER {
                            // macOS: scanning must be (re)requested once the radio is ready.
                            if central.start_scan(ScanFilter::default()).await.is_err() {
                                break Err("error");
                            }
                            powered = true;
                            quiet_since = now_ms();
                            set_status(core, "scanning");
                        } else if !powered {
                            // Windows: the watcher was aborted with the radio; start over.
                            break Ok(End::Requested);
                        }
                    }
                    CentralEvent::ManufacturerDataAdvertisement { id, manufacturer_data } => {
                        if let Some(data) = manufacturer_data.get(&MANUFACTURER_ID) {
                            ecoflow.insert(id.clone(), ());
                            let rssi = match central.peripheral(&id).await {
                                Ok(p) => p.properties().await.ok().flatten().and_then(|pr| pr.rssi),
                                Err(_) => None,
                            };
                            handle(core, data, rssi);
                        }
                    }
                    CentralEvent::DeviceDiscovered(id) => read_props(&central, &id, core).await,
                    CentralEvent::DeviceUpdated(id) => {
                        if ecoflow.contains_key(&id) {
                            read_props(&central, &id, core).await;
                        }
                    }
                    _ => {}
                }
            }
        }
    };
    let _ = central.stop_scan().await;
    result
}
