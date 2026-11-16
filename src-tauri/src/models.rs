//! EcoFlow model detection by serial-number prefix.
//!
//! Prefixes come from the community reverse-engineering projects
//! (rabits/ha-ef-ble, npike/ha-ecoflow-ble, tolwi/hassio-ecoflow-cloud).

/// How a model can be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Caps {
    /// Battery percentage is present in the BLE advertisement (no connection needed).
    pub ble_soc: bool,
    /// The cloud (app login + MQTT) sends JSON telemetry we can parse.
    pub cloud_json: bool,
}

struct Model {
    prefixes: &'static [&'static str],
    name: &'static str,
    caps: Caps,
}

const GEN2: Caps = Caps { ble_soc: true, cloud_json: true };
const GEN3_BLE: Caps = Caps { ble_soc: true, cloud_json: false };
const OTHER: Caps = Caps { ble_soc: false, cloud_json: false };
/// Unknown prefix: no BLE percentage, but the cloud is still tried (older
/// JSON devices like River Pro / Delta Max are not in the table).
const UNKNOWN: Caps = Caps { ble_soc: false, cloud_json: true };

const MODELS: &[Model] = &[
    // River 2 family
    Model { prefixes: &["R601", "R603"], name: "River 2", caps: GEN2 },
    Model { prefixes: &["R611", "R613"], name: "River 2 Max", caps: GEN2 },
    Model { prefixes: &["R621", "R623"], name: "River 2 Pro", caps: GEN2 },
    // Delta 2 family
    Model { prefixes: &["R331", "R335"], name: "Delta 2", caps: GEN2 },
    Model { prefixes: &["R351", "R354"], name: "Delta 2 Max", caps: GEN2 },
    Model { prefixes: &["R701"], name: "Delta 2", caps: GEN2 },
    // River 3 (BLE percentage confirmed by npike/ha-ecoflow-ble; cloud uses protobuf)
    Model { prefixes: &["R651", "R653", "R654", "R655"], name: "River 3", caps: GEN3_BLE },
    // Recognised, but BLE percentage is not known to be in the advert and the
    // cloud uses protobuf (not parsed in this version)
    Model { prefixes: &["R631", "R634", "R635"], name: "River 3 Plus", caps: OTHER },
    Model { prefixes: &["P231"], name: "Delta 3", caps: OTHER },
    Model { prefixes: &["P321", "P322"], name: "Delta 3 Classic", caps: OTHER },
    Model { prefixes: &["P351"], name: "Delta 3 Plus", caps: OTHER },
    Model { prefixes: &["D3N1"], name: "Delta 3 Max", caps: OTHER },
    Model { prefixes: &["D3M1"], name: "Delta 3 Max Plus", caps: OTHER },
    Model { prefixes: &["MR51", "MR54"], name: "Delta Pro 3", caps: OTHER },
    Model { prefixes: &["Y711"], name: "Delta Pro Ultra", caps: OTHER },
];

fn find(sn: &str) -> Option<&'static Model> {
    MODELS
        .iter()
        .find(|m| m.prefixes.iter().any(|p| sn.starts_with(p)))
}

pub fn name(sn: &str) -> String {
    find(sn).map(|m| m.name.to_string()).unwrap_or_else(|| "EcoFlow".to_string())
}

pub fn caps(sn: &str) -> Caps {
    find(sn).map(|m| m.caps).unwrap_or(UNKNOWN)
}

/// Parsed EcoFlow BLE advertisement (manufacturer id 0xB5B5).
#[derive(Clone, Debug, PartialEq)]
pub struct Advert {
    pub serial: String,
    /// `None` when the model is not known to carry the percentage.
    pub soc: Option<u8>,
}

pub const MANUFACTURER_ID: u16 = 0xB5B5;

/// Layout: [0] protocol version, [1..17] serial (ASCII), [17] status byte.
/// For River 2 / Delta 2 / River 3 the low 7 bits of the status byte are the
/// battery percentage, bit 7 is an "active" flag.
pub fn parse_advert(data: &[u8]) -> Option<Advert> {
    if data.len() < 18 {
        return None;
    }
    let serial_bytes = &data[1..17];
    if !serial_bytes.iter().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let serial = String::from_utf8(serial_bytes.to_vec()).ok()?;
    let raw = data[17] & 0x7F;
    let soc = if caps(&serial).ble_soc && raw <= 100 { Some(raw) } else { None };
    Some(Advert { serial, soc })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_river2_pro() {
        let mut d = vec![0x01u8];
        d.extend_from_slice(b"R621ZEB4ZE123456");
        d.push(0x80 | 78);
        d.extend_from_slice(&[0, 0, 0, 0, 0]);
        let a = parse_advert(&d).unwrap();
        assert_eq!(a.serial, "R621ZEB4ZE123456");
        assert_eq!(a.soc, Some(78));
        assert_eq!(name(&a.serial), "River 2 Pro");
    }

    #[test]
    fn unknown_model_has_no_soc() {
        let mut d = vec![0x01u8];
        d.extend_from_slice(b"P231ZEB4ZE123456");
        d.push(55);
        let a = parse_advert(&d).unwrap();
        assert_eq!(a.soc, None);
        assert_eq!(name(&a.serial), "Delta 3");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_advert(&[0u8; 10]).is_none());
        let mut d = vec![0x01u8];
        d.extend_from_slice(&[0xFF; 16]);
        d.push(50);
        assert!(parse_advert(&d).is_none());
    }
}
