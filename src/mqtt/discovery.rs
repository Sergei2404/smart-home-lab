use super::topics::Topics;
use crate::domain::{Device, DeviceId, DeviceKind, State};
use serde_json::{Value, json};

pub const ONLINE: &str = "online";
pub const OFFLINE: &str = "offline";

/// Home Assistant entity platforms used for devices; all of them are cleared when a device is removed.
pub const COMPONENTS: [&str; 3] = ["binary_sensor", "switch", "sensor"];

pub fn component(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Sensor => "binary_sensor",
        DeviceKind::Switch | DeviceKind::Relay => "switch",
        DeviceKind::Percent | DeviceKind::Temperature => "sensor",
    }
}

/// Whether a retained discovery config is one this bridge published for `id`: our `unique_id` and a
/// state topic under our base topic. Another integration's entities, and those of a lab that uses
/// another base topic, are not ours and must never be touched.
pub fn is_ours(topics: &Topics, id: &DeviceId, payload: &[u8]) -> bool {
    let Ok(config) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    config["unique_id"] == format!("shl_{id}") && config["state_topic"] == topics.state(id)
}

/// `ON` / `OFF` for binary devices, the plain decimal number for numeric sensors.
pub fn state_payload(state: State) -> String {
    match state {
        State::Bool(true) => "ON".to_owned(),
        State::Bool(false) => "OFF".to_owned(),
        State::Int(value) => value.to_string(),
    }
}

/// Parses a command payload; Home Assistant sends exactly `ON` / `OFF`.
pub fn parse_command(payload: &[u8]) -> Option<bool> {
    let payload = std::str::from_utf8(payload).ok()?.trim();
    if payload.eq_ignore_ascii_case("on") {
        Some(true)
    } else if payload.eq_ignore_ascii_case("off") {
        Some(false)
    } else {
        None
    }
}

/// MQTT Discovery config: every device is its own Home Assistant device with one entity named after
/// it (`"name": null`), available only while both the bridge and the device are online.
pub fn config_payload(topics: &Topics, device: &Device) -> Value {
    let unique_id = format!("shl_{}", device.id);
    let model = match device.kind {
        DeviceKind::Sensor => "Virtual sensor",
        DeviceKind::Switch => "Virtual switch",
        DeviceKind::Relay => "Virtual relay",
        DeviceKind::Percent => "Virtual percent sensor",
        DeviceKind::Temperature => "Virtual temperature sensor",
    };
    let mut payload = json!({
        "name": null,
        "unique_id": unique_id,
        "state_topic": topics.state(&device.id),
        "availability": [
            { "topic": topics.status() },
            { "topic": topics.availability(&device.id) },
        ],
        "availability_mode": "all",
        "device": {
            "identifiers": [unique_id],
            "name": device.name,
            "manufacturer": "Smart Home Lab",
            "model": model,
        },
    });
    let object = payload.as_object_mut().expect("payload is an object");
    match device.kind {
        DeviceKind::Sensor => {}
        DeviceKind::Switch | DeviceKind::Relay => {
            object.insert("command_topic".into(), topics.command(&device.id).into());
            let class = if device.kind == DeviceKind::Relay {
                "outlet"
            } else {
                "switch"
            };
            object.insert("device_class".into(), class.into());
        }
        DeviceKind::Percent => {
            object.insert("unit_of_measurement".into(), "%".into());
            object.insert("state_class".into(), "measurement".into());
        }
        DeviceKind::Temperature => {
            object.insert("device_class".into(), "temperature".into());
            object.insert("unit_of_measurement".into(), "°C".into());
            object.insert("state_class".into(), "measurement".into());
        }
    }
    payload
}

#[cfg(test)]
mod tests;
