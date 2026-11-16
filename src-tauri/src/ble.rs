//! Passive BLE scanner: reads the EcoFlow advertisement (no pairing, no connection).

use crate::core::{now_ms, BleSeen, Shared, BLE_FRESH_MS};
use crate::models::{parse_advert, MANUFACTURER_ID};
use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, PeripheralId};
use futures::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

/// Rebuild the scan session after this long without any EcoFlow advert.
const WATCHDOG_MS: u64 = 3 * 60 * 1000;

fn set_status(core: &Shared, status: &str) {
    let mut ble = core.ble.lock().unwrap();
    if ble.status != status {
        ble.status = status.to_string();
        drop(ble);
        core.mark_changed();
    }
}

pub async fn run(core: Shared) {
    loop {
        if !core.settings().ble_enabled {
            set_status(&core, "off");
            core.ble_restart.notified().await;
            continue;
        }
        set_status(&core, "starting");
        match scan_session(&core).await {
            Ok(()) => {} // restarted on purpose
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

async fn adapter() -> Result<Adapter, &'static str> {
    let manager = Manager::new().await.map_err(|e| {
        log::warn!("BLE manager: {e}");
        "no_adapter"
    })?;
    let adapters = manager.adapters().await.map_err(|_| "no_adapter")?;
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

async fn scan_session(core: &Shared) -> Result<(), &'static str> {
    let central = adapter().await?;
    let mut events = central.events().await.map_err(|_| "error")?;
    central.start_scan(ScanFilter::default()).await.map_err(|e| {
        log::warn!("BLE start_scan: {e}");
        "error"
    })?;
    set_status(core, "scanning");

    // Remember which peripherals are EcoFlow so RSSI updates are cheap to follow.
    let mut ecoflow: HashMap<PeripheralId, ()> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    let session_start = now_ms();
    let result = loop {
        tokio::select! {
            _ = core.ble_restart.notified() => break Ok(()),
            _ = tick.tick() => {
                // Lets the popup notice devices going away / values getting old.
                core.mark_changed();
                // Some stacks stop scanning silently (e.g. after sleep). Calling start_scan
                // again is not safe (on Windows btleplug adds another event handler each
                // time), so when nothing EcoFlow was heard for a while, rebuild the session.
                let last_adv = core.ble.lock().unwrap().devices.values().map(|d| d.last).max().unwrap_or(0);
                if now_ms().saturating_sub(last_adv.max(session_start)) > WATCHDOG_MS {
                    break Ok(());
                }
            }
            ev = events.next() => {
                let Some(ev) = ev else { break Err("error") };
                match ev {
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
