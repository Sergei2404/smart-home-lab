use super::*;

const ALL_KINDS: [DeviceKind; 5] = [
    DeviceKind::Sensor,
    DeviceKind::Switch,
    DeviceKind::Relay,
    DeviceKind::Percent,
    DeviceKind::Temperature,
];

fn topics() -> Topics {
    Topics::new("homeassistant", "smarthomelab").unwrap()
}

#[test]
fn components_by_kind() {
    assert_eq!(component(DeviceKind::Sensor), "binary_sensor");
    assert_eq!(component(DeviceKind::Switch), "switch");
    assert_eq!(component(DeviceKind::Relay), "switch");
    assert_eq!(component(DeviceKind::Percent), "sensor");
    assert_eq!(component(DeviceKind::Temperature), "sensor");
    for kind in ALL_KINDS {
        assert!(COMPONENTS.contains(&component(kind)), "{kind:?}");
    }
}

#[test]
fn sensor_config_has_no_command_topic() {
    let device = Device::new("Hall Motion", DeviceKind::Sensor).unwrap();
    assert_eq!(
        config_payload(&topics(), &device),
        json!({
            "name": null,
            "unique_id": "shl_hall-motion",
            "state_topic": "smarthomelab/hall-motion/state",
            "availability": [
                { "topic": "smarthomelab/status" },
                { "topic": "smarthomelab/hall-motion/availability" },
            ],
            "availability_mode": "all",
            "device": {
                "identifiers": ["shl_hall-motion"],
                "name": "Hall Motion",
                "manufacturer": "Smart Home Lab",
                "model": "Virtual sensor",
            },
        })
    );
}

#[test]
fn switch_config_accepts_commands() {
    let device = Device::new("Living Lamp", DeviceKind::Switch).unwrap();
    let payload = config_payload(&topics(), &device);
    assert_eq!(payload["command_topic"], "smarthomelab/living-lamp/set");
    assert_eq!(payload["device_class"], "switch");
    assert_eq!(payload["device"]["model"], "Virtual switch");
}

#[test]
fn relay_is_an_outlet() {
    let device = Device::new("Heater", DeviceKind::Relay).unwrap();
    let payload = config_payload(&topics(), &device);
    assert_eq!(payload["device_class"], "outlet");
    assert_eq!(payload["command_topic"], "smarthomelab/heater/set");
}

#[test]
fn temperature_config_has_a_unit_and_no_command_topic() {
    let device = Device::new("Outside", DeviceKind::Temperature).unwrap();
    assert_eq!(
        config_payload(&topics(), &device),
        json!({
            "name": null,
            "unique_id": "shl_outside",
            "state_topic": "smarthomelab/outside/state",
            "availability": [
                { "topic": "smarthomelab/status" },
                { "topic": "smarthomelab/outside/availability" },
            ],
            "availability_mode": "all",
            "device_class": "temperature",
            "unit_of_measurement": "°C",
            "state_class": "measurement",
            "device": {
                "identifiers": ["shl_outside"],
                "name": "Outside",
                "manufacturer": "Smart Home Lab",
                "model": "Virtual temperature sensor",
            },
        })
    );
}

#[test]
fn percent_config_is_a_plain_measurement_in_percent() {
    let device = Device::new("Humidity", DeviceKind::Percent).unwrap();
    let payload = config_payload(&topics(), &device);
    assert_eq!(payload["unit_of_measurement"], "%");
    assert_eq!(payload["state_class"], "measurement");
    assert!(payload.get("device_class").is_none());
    assert!(payload.get("command_topic").is_none());
}

#[test]
fn only_controllable_kinds_get_a_command_topic() {
    for kind in ALL_KINDS {
        let device = Device::new("x", kind).unwrap();
        let has_command = config_payload(&topics(), &device)
            .get("command_topic")
            .is_some();
        assert_eq!(has_command, kind.accepts_commands(), "{kind:?}");
    }
}

#[test]
fn every_kind_is_available_only_when_the_bridge_and_the_device_are() {
    for kind in ALL_KINDS {
        let device = Device::new("Thing", kind).unwrap();
        let payload = config_payload(&topics(), &device);
        assert_eq!(
            payload["availability"],
            json!([
                { "topic": "smarthomelab/status" },
                { "topic": "smarthomelab/thing/availability" },
            ]),
            "{kind:?}"
        );
        assert_eq!(payload["availability_mode"], "all", "{kind:?}");
        assert!(payload.get("availability_topic").is_none(), "{kind:?}");
    }
}

#[test]
fn state_payloads() {
    assert_eq!(state_payload(State::Bool(true)), "ON");
    assert_eq!(state_payload(State::Bool(false)), "OFF");
    assert_eq!(state_payload(State::Int(57)), "57");
    assert_eq!(state_payload(State::Int(-12)), "-12");
    assert_eq!(state_payload(State::Int(0)), "0");
}

#[test]
fn command_payloads() {
    for (payload, expected) in [
        (&b"ON"[..], Some(true)),
        (b"OFF", Some(false)),
        (b"on", Some(true)),
        (b" off\n", Some(false)),
        (b"TOGGLE", None),
        (b"", None),
        (&[0xff, 0xfe], None),
    ] {
        assert_eq!(parse_command(payload), expected, "{payload:?}");
    }
}

#[test]
fn only_our_own_configs_are_recognised() {
    let t = topics();
    let lamp = Device::new("Lamp", DeviceKind::Switch).unwrap();
    let ours = config_payload(&t, &lamp).to_string();
    assert!(is_ours(&t, &lamp.id, ours.as_bytes()));

    let other_lab = Topics::new("homeassistant", "another-lab").unwrap();
    let theirs = config_payload(&other_lab, &lamp).to_string();
    assert!(
        !is_ours(&t, &lamp.id, theirs.as_bytes()),
        "another base topic"
    );

    let foreign = r#"{"unique_id":"zigbee_lamp","state_topic":"smarthomelab/lamp/state"}"#;
    assert!(
        !is_ours(&t, &lamp.id, foreign.as_bytes()),
        "another unique_id"
    );
    let other_id = DeviceId::parse("door").unwrap();
    assert!(!is_ours(&t, &other_id, ours.as_bytes()), "another device");
    for garbage in ["", "not json", "[]", "null", "{}"] {
        assert!(!is_ours(&t, &lamp.id, garbage.as_bytes()), "{garbage:?}");
    }
}
