use super::*;
use crate::domain::{DeviceKind, State};

fn set(device: &DeviceId, state: impl Into<State>) -> Step {
    Step::Set {
        device: device.clone(),
        state: state.into(),
    }
}

fn snapshot_error(snapshot: Snapshot) -> String {
    snapshot.validate().unwrap_err().to_string()
}

fn many_devices(count: usize) -> Vec<Device> {
    (0..count)
        .map(|i| Device::new(&format!("Device {i}"), DeviceKind::Sensor).unwrap())
        .collect()
}

#[test]
fn consistent_snapshot_is_valid() {
    let lamp = Device::new("Lamp", DeviceKind::Switch).unwrap();
    let night = Scenario::new("Night", false, vec![set(&lamp.id, true)]).unwrap();
    let snapshot = Snapshot {
        devices: vec![lamp],
        scenarios: vec![night],
    };

    assert!(snapshot.validate().is_ok());
    assert!(Snapshot::default().validate().is_ok());
}

#[test]
fn validation_reports_the_offending_item() {
    let lamp = Device::new("Lamp", DeviceKind::Switch).unwrap();
    let humidity = Device::new("Humidity", DeviceKind::Percent).unwrap();

    let mut broken = humidity.clone();
    broken.state = State::Int(500);
    assert_eq!(
        snapshot_error(Snapshot {
            devices: vec![broken],
            ..Snapshot::default()
        }),
        "device humidity: value 500 is outside the range 0..=100"
    );

    assert_eq!(
        snapshot_error(Snapshot {
            devices: vec![lamp.clone(), lamp.clone()],
            ..Snapshot::default()
        }),
        "device id lamp appears more than once"
    );

    let night = Scenario::new("Night", false, vec![set(&lamp.id, true)]).unwrap();
    assert_eq!(
        snapshot_error(Snapshot {
            devices: vec![lamp.clone()],
            scenarios: vec![night.clone(), night.clone()],
        }),
        "scenario id night appears more than once"
    );

    let mut spinning = Scenario::new(
        "Spin",
        true,
        vec![set(&lamp.id, true), Step::Delay { secs: 1 }],
    )
    .unwrap();
    spinning.steps.pop();
    assert_eq!(
        snapshot_error(Snapshot {
            devices: vec![lamp.clone()],
            scenarios: vec![spinning],
        }),
        "scenario spin: a repeating scenario needs at least one delay step"
    );

    let mut mismatch = Scenario::new("Mismatch", false, vec![set(&lamp.id, true)]).unwrap();
    mismatch.steps = vec![set(&lamp.id, 5)];
    assert_eq!(
        snapshot_error(Snapshot {
            devices: vec![lamp],
            scenarios: vec![mismatch],
        }),
        "scenario mismatch: this device needs an on/off state"
    );
}

#[test]
fn validation_tolerates_steps_naming_missing_devices() {
    let ghost = DeviceId::parse("ghost").unwrap();
    let scenario = Scenario::new("Old", false, vec![set(&ghost, true)]).unwrap();
    let snapshot = Snapshot {
        scenarios: vec![scenario],
        ..Snapshot::default()
    };
    assert!(snapshot.validate().is_ok());
}

#[test]
fn a_snapshot_over_the_limits_is_refused_and_one_at_the_limits_is_fine() {
    let at_limit = Snapshot {
        devices: many_devices(MAX_DEVICES),
        ..Snapshot::default()
    };
    assert!(at_limit.validate().is_ok());

    let mut too_many_devices = at_limit.clone();
    too_many_devices
        .devices
        .push(Device::new("One more", DeviceKind::Sensor).unwrap());
    let message = too_many_devices.validate().unwrap_err().to_string();
    assert!(message.contains("at most 500 devices"), "{message}");
    assert!(message.contains("has 501"), "{message}");

    let steps = vec![set(&DeviceId::parse("x").unwrap(), true)];
    let scenarios = |count: usize| -> Vec<Scenario> {
        (0..count)
            .map(|i| {
                Scenario::with_id(
                    ScenarioId::parse(&format!("s{i}")).unwrap(),
                    "Spin",
                    false,
                    steps.clone(),
                )
                .unwrap()
            })
            .collect()
    };
    assert!(
        Snapshot {
            scenarios: scenarios(MAX_SCENARIOS),
            ..Snapshot::default()
        }
        .validate()
        .is_ok()
    );
    assert!(matches!(
        Snapshot {
            scenarios: scenarios(MAX_SCENARIOS + 1),
            ..Snapshot::default()
        }
        .validate(),
        Err(SnapshotError::TooMany {
            what: "scenarios",
            ..
        })
    ));
}
