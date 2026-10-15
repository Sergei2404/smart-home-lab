use super::*;

const ALL_KINDS: [DeviceKind; 5] = [
    DeviceKind::Sensor,
    DeviceKind::Switch,
    DeviceKind::Relay,
    DeviceKind::Percent,
    DeviceKind::Temperature,
];

#[test]
fn id_is_derived_from_the_name() {
    assert_eq!(
        DeviceId::from_name("Living Lamp").unwrap().as_str(),
        "living-lamp"
    );
    assert_eq!(DeviceId::from_name("Лампа"), Err(DomainError::EmptySlug));
}

#[test]
fn id_parse_validates() {
    assert!(DeviceId::parse("lamp-1").is_ok());
    assert_eq!(
        DeviceId::parse("Bad Id"),
        Err(DomainError::InvalidId("Bad Id".to_owned()))
    );
}

#[test]
fn id_deserialization_validates() {
    assert!(serde_json::from_str::<DeviceId>(r#""lamp""#).is_ok());
    assert!(serde_json::from_str::<DeviceId>(r#""Bad Id""#).is_err());
}

#[test]
fn id_serializes_as_a_plain_string() {
    let id = DeviceId::parse("lamp").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), r#""lamp""#);
    assert_eq!(id.to_string(), "lamp");
}

#[test]
fn new_device_starts_with_the_initial_state_and_a_trimmed_name() {
    let device = Device::new("  Living Lamp ", DeviceKind::Switch).unwrap();
    assert_eq!(device.name, "Living Lamp");
    assert_eq!(device.id.as_str(), "living-lamp");
    assert_eq!(device.state, State::Bool(false));

    for kind in [DeviceKind::Percent, DeviceKind::Temperature] {
        assert_eq!(Device::new("x", kind).unwrap().state, State::Int(0));
    }
}

#[test]
fn invalid_names_are_rejected() {
    for name in ["", "   ", "tab\there", &"a".repeat(65)] {
        assert_eq!(
            Device::new(name, DeviceKind::Sensor),
            Err(DomainError::InvalidName),
            "{name:?}"
        );
    }
}

#[test]
fn kind_uses_snake_case() {
    assert_eq!(
        serde_json::to_string(&DeviceKind::Relay).unwrap(),
        r#""relay""#
    );
    assert_eq!(
        serde_json::to_string(&DeviceKind::Temperature).unwrap(),
        r#""temperature""#
    );
}

#[test]
fn initial_state_matches_the_kind() {
    for kind in ALL_KINDS {
        assert_eq!(kind.check(kind.initial_state()), Ok(kind.initial_state()));
    }
}

#[test]
fn binary_kinds_accept_only_on_off() {
    for kind in [DeviceKind::Sensor, DeviceKind::Switch, DeviceKind::Relay] {
        assert!(kind.check(State::Bool(true)).is_ok());
        assert_eq!(
            kind.check(State::Int(1)),
            Err(DomainError::StateMismatch("an on/off state"))
        );
    }
}

#[test]
fn numeric_kinds_reject_on_off() {
    for kind in [DeviceKind::Percent, DeviceKind::Temperature] {
        assert_eq!(
            kind.check(State::Bool(true)),
            Err(DomainError::StateMismatch("a whole number"))
        );
    }
}

#[test]
fn percent_is_limited_to_0_100() {
    for value in [0, 1, 50, 100] {
        assert!(
            DeviceKind::Percent.check(State::Int(value)).is_ok(),
            "{value}"
        );
    }
    for value in [-1, 101, i32::MIN, i32::MAX] {
        assert_eq!(
            DeviceKind::Percent.check(State::Int(value)),
            Err(DomainError::OutOfRange {
                value,
                min: 0,
                max: 100
            }),
            "{value}"
        );
    }
}

#[test]
fn temperature_is_unbounded_and_may_be_negative() {
    for value in [-273, -40, 0, 36, 1000, i32::MIN, i32::MAX] {
        assert!(
            DeviceKind::Temperature.check(State::Int(value)).is_ok(),
            "{value}"
        );
    }
}

#[test]
fn only_switches_and_relays_accept_commands() {
    for kind in ALL_KINDS {
        let expected = matches!(kind, DeviceKind::Switch | DeviceKind::Relay);
        assert_eq!(kind.accepts_commands(), expected, "{kind:?}");
    }
}

#[test]
fn validate_catches_what_the_api_would_reject() {
    let mut device = Device::new("Humidity", DeviceKind::Percent).unwrap();
    assert!(device.validate().is_ok());

    device.state = State::Int(101);
    assert!(matches!(
        device.validate(),
        Err(DomainError::OutOfRange { .. })
    ));
    device.state = State::Bool(true);
    assert!(matches!(
        device.validate(),
        Err(DomainError::StateMismatch(_))
    ));
    device.state = State::Int(1);
    device.name = "  ".to_owned();
    assert_eq!(device.validate(), Err(DomainError::InvalidName));
}

#[test]
fn new_devices_are_online() {
    assert!(Device::new("Lamp", DeviceKind::Switch).unwrap().online);
}

#[test]
fn a_device_without_the_online_field_is_online() {
    let hand_written = r#"{"id":"lamp","name":"Lamp","kind":"switch","state":true}"#;
    assert!(serde_json::from_str::<Device>(hand_written).unwrap().online);

    let offline = r#"{"id":"lamp","name":"Lamp","kind":"switch","state":true,"online":false}"#;
    assert!(!serde_json::from_str::<Device>(offline).unwrap().online);
}
