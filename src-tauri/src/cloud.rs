//! EcoFlow cloud via the app login (same endpoints the EcoFlow app uses):
//! POST /auth/login -> GET /iot-auth/app/certification -> MQTT over TLS.
//! Telemetry is JSON for River 2 / Delta 2 and older models.

use crate::core::{now_ms, Shared};
use crate::models;
use crate::settings;
use base64::Engine;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, Transport};
use serde_json::{json, Value};
use std::time::Duration;

struct Login {
    user_id: String,
    mqtt_host: String,
    mqtt_port: u16,
    mqtt_user: String,
    mqtt_pass: String,
}

enum Fail {
    Auth(String),
    Other(String),
}

fn set(core: &Shared, status: &str, message: &str) {
    let mut c = core.cloud.lock().unwrap();
    if c.status != status || c.message != message {
        c.status = status.into();
        c.message = message.into();
        drop(c);
        core.mark_changed();
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("http client")
}

fn api_ok(v: &Value) -> Result<(), String> {
    let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
    if msg.eq_ignore_ascii_case("success") {
        Ok(())
    } else if msg.is_empty() {
        Err("unexpected response".into())
    } else {
        Err(msg.to_string())
    }
}

async fn login(host: &str, email: &str, password: &str) -> Result<Login, Fail> {
    let client = http();
    let resp = client
        .post(format!("https://{host}/auth/login"))
        .header("lang", "en_US")
        .json(&json!({
            "email": email,
            "password": base64::engine::general_purpose::STANDARD.encode(password),
            "scene": "IOT_APP",
            "userType": "ECOFLOW",
        }))
        .send()
        .await
        .map_err(|e| Fail::Other(e.to_string()))?;
    let v: Value = resp.json().await.map_err(|e| Fail::Other(e.to_string()))?;
    if let Err(m) = api_ok(&v) {
        // Any error code from /auth/login means the credentials were rejected.
        return Err(if v.get("code").is_some() { Fail::Auth(m) } else { Fail::Other(m) });
    }
    let token = v["data"]["token"].as_str().ok_or(Fail::Other("no token".into()))?.to_string();
    let user_id = match &v["data"]["user"]["userId"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err(Fail::Other("no userId".into())),
    };

    let resp = client
        .get(format!("https://{host}/iot-auth/app/certification"))
        .header("lang", "en_US")
        .bearer_auth(&token)
        .query(&[("userId", &user_id)])
        .send()
        .await
        .map_err(|e| Fail::Other(e.to_string()))?;
    let v: Value = resp.json().await.map_err(|e| Fail::Other(e.to_string()))?;
    api_ok(&v).map_err(Fail::Other)?;
    let d = &v["data"];
    let port = match &d["port"] {
        Value::String(s) => s.parse().unwrap_or(8883),
        Value::Number(n) => n.as_u64().unwrap_or(8883) as u16,
        _ => 8883,
    };
    Ok(Login {
        user_id,
        mqtt_host: d["url"].as_str().ok_or(Fail::Other("no mqtt url".into()))?.to_string(),
        mqtt_port: port,
        mqtt_user: d["certificateAccount"].as_str().unwrap_or_default().to_string(),
        mqtt_pass: d["certificatePassword"].as_str().unwrap_or_default().to_string(),
    })
}

fn quota_request() -> Vec<u8> {
    let id = (now_ms() % 1_000_000_000).to_string();
    json!({
        "from": "Android",
        "id": id,
        "version": "1.1",
        "moduleType": 0,
        "operateType": "latestQuotas",
        "params": {}
    })
    .to_string()
    .into_bytes()
}

/// Merge an incoming message into the parameter map. Returns `Some(online)` when
/// the message tells whether the station is connected to the cloud.
fn apply_message(core: &Shared, sn: &str, data_topic: &str, payload: &[u8], topic: &str) -> Option<bool> {
    let v: Value = serde_json::from_slice(payload).ok()?;
    let mut c = core.cloud.lock().unwrap();
    if c.sn != sn {
        return None;
    }
    if topic == data_topic {
        let params = v.get("params")?.as_object()?;
        if params.is_empty() {
            return None;
        }
        for (k, val) in params {
            c.params.insert(k.clone(), val.clone());
        }
        c.last = now_ms();
        return Some(true);
    }
    // get_reply
    if v.get("operateType").and_then(|o| o.as_str()) == Some("latestQuotas") {
        let data = v.get("data")?;
        let online = match &data["online"] {
            Value::Number(n) => n.as_i64() == Some(1),
            Value::Bool(b) => *b,
            Value::String(s) => s == "1",
            _ => false,
        };
        if online {
            if let Some(map) = data.get("quotaMap").and_then(|m| m.as_object()) {
                for (k, val) in map {
                    c.params.insert(k.clone(), val.clone());
                }
                c.last = now_ms();
            }
        }
        return Some(online);
    }
    None
}

pub async fn run(core: Shared) {
    let mut backoff = 10u64;
    loop {
        let s = core.settings();
        if !s.cloud_enabled {
            set(&core, "off", "");
            core.cloud_restart.notified().await;
            continue;
        }
        let password = settings::get_password(&s.cloud_email);
        if s.cloud_email.is_empty() || password.is_none() {
            set(&core, "no_login", "");
            core.cloud_restart.notified().await;
            continue;
        }
        let sn = core.effective_sn();
        if sn.is_empty() {
            set(&core, "no_device", "");
            // Wait for a BLE station to appear or for new settings.
            tokio::select! {
                _ = core.cloud_restart.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
            }
            continue;
        }
        {
            let mut c = core.cloud.lock().unwrap();
            if c.sn != sn {
                c.sn = sn.clone();
                c.params.clear();
                c.last = 0;
            }
        }
        if !models::caps(&sn).cloud_json {
            set(&core, "unsupported", "");
            core.cloud_restart.notified().await;
            continue;
        }

        set(&core, "connecting", "");
        let result = session(&core, &s.api_host().to_string(), &s.cloud_email, &password.unwrap(), &sn).await;
        match result {
            Ok(()) => backoff = 10, // restart requested
            Err(Fail::Auth(m)) => {
                set(&core, "auth_error", &m);
                core.cloud_restart.notified().await;
            }
            Err(Fail::Other(m)) => {
                log::warn!("cloud: {m}");
                set(&core, "error", &m);
                tokio::select! {
                    _ = core.cloud_restart.notified() => { backoff = 10; }
                    _ = tokio::time::sleep(Duration::from_secs(backoff)) => {
                        backoff = (backoff * 2).min(300);
                    }
                }
            }
        }
    }
}

async fn session(core: &Shared, host: &str, email: &str, password: &str, sn: &str) -> Result<(), Fail> {
    let l = login(host, email, password).await?;
    let client_id = format!(
        "ANDROID_{}_{}",
        uuid::Uuid::new_v4().simple().to_string().to_uppercase(),
        l.user_id
    );
    let mut opts = MqttOptions::new(client_id, l.mqtt_host.clone(), l.mqtt_port);
    opts.set_credentials(l.mqtt_user.clone(), l.mqtt_pass.clone());
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);
    opts.set_transport(Transport::tls_with_default_config());

    let data_topic = format!("/app/device/property/{sn}");
    let get_topic = format!("/app/{}/{sn}/thing/property/get", l.user_id);
    let reply_topic = format!("/app/{}/{sn}/thing/property/get_reply", l.user_id);

    let (client, mut eventloop) = AsyncClient::new(opts, 20);
    let mut quota_tick = tokio::time::interval(Duration::from_secs(60));
    let mut sn_check = tokio::time::interval(Duration::from_secs(10));
    let mut connected = false;
    let mut errors = 0u32;
    // If nothing arrives for this long after connecting, the station is offline.
    let mut last_rx = now_ms();

    let result = loop {
        tokio::select! {
            _ = core.cloud_restart.notified() => break Ok(()),
            _ = sn_check.tick() => {
                if core.effective_sn() != sn {
                    break Ok(());
                }
                if connected && now_ms().saturating_sub(last_rx) > 3 * 60_000 {
                    set(core, "offline", "");
                }
            }
            _ = quota_tick.tick(), if connected => {
                let _ = client.publish(get_topic.clone(), QoS::AtMostOnce, false, quota_request()).await;
            }
            ev = eventloop.poll() => match ev {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    connected = true;
                    errors = 0;
                    let _ = client.subscribe(data_topic.clone(), QoS::AtMostOnce).await;
                    let _ = client.subscribe(reply_topic.clone(), QoS::AtMostOnce).await;
                    let _ = client.publish(get_topic.clone(), QoS::AtMostOnce, false, quota_request()).await;
                }
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    if let Some(online) = apply_message(core, sn, &data_topic, &p.payload, &p.topic) {
                        if online {
                            last_rx = now_ms();
                            set(core, "online", "");
                        } else {
                            set(core, "offline", "");
                        }
                        core.mark_changed();
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    connected = false;
                    errors += 1;
                    let msg = e.to_string();
                    if msg.contains("NotAuthorized") || msg.contains("BadUserNamePassword") {
                        break Err(Fail::Other(format!("MQTT: {msg}")));
                    }
                    if errors >= 3 {
                        // Re-login to refresh the MQTT certificate.
                        break Err(Fail::Other(format!("MQTT: {msg}")));
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    };
    let _ = client.disconnect().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Core;
    use crate::settings::Settings;

    #[test]
    fn merges_data_and_reply() {
        let core = Core::new(Settings::default(), std::env::temp_dir().join("sb-test.json"));
        core.cloud.lock().unwrap().sn = "R621X".into();
        let data = br#"{"params":{"pd.soc":77,"pd.wattsInSum":10}}"#;
        assert_eq!(apply_message(&core, "R621X", "/app/device/property/R621X", data, "/app/device/property/R621X"), Some(true));
        let reply = br#"{"operateType":"latestQuotas","data":{"online":1,"quotaMap":{"pd.wattsOutSum":5}}}"#;
        assert_eq!(apply_message(&core, "R621X", "/app/device/property/R621X", reply, "/app/1/R621X/thing/property/get_reply"), Some(true));
        let c = core.cloud.lock().unwrap();
        assert_eq!(c.params.len(), 3);
        let offline = br#"{"operateType":"latestQuotas","data":{"online":0}}"#;
        drop(c);
        assert_eq!(apply_message(&core, "R621X", "/app/device/property/R621X", offline, "/x"), Some(false));
    }
}
