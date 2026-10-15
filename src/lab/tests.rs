use super::*;
use tokio::sync::broadcast::error::TryRecvError;

fn lab_with(name: &str, kind: DeviceKind) -> (Lab, DeviceId) {
    let lab = Lab::new(Snapshot::default());
    let id = lab.add_device(name, kind).unwrap().id;
    (lab, id)
}

fn set(device: &DeviceId, state: impl Into<State>) -> Step {
    Step::Set {
        device: device.clone(),
        state: state.into(),
    }
}

#[test]
fn add_device_emits_event_and_updates_snapshot() {
    let lab = Lab::new(Snapshot::default());
    let mut rx = lab.subscribe();

    let device = lab.add_device("Lamp", DeviceKind::Switch).unwrap();

    assert_eq!(lab.devices(), vec![device.clone()]);
    assert_eq!(lab.snapshot().devices, vec![device.clone()]);
    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::DeviceAdded { device }
    );
}

#[test]
fn duplicate_device_is_rejected() {
    let (lab, _) = lab_with("Lamp", DeviceKind::Switch);
    assert!(matches!(
        lab.add_device("lamp", DeviceKind::Sensor),
        Err(LabError::AlreadyExists { .. })
    ));
    assert_eq!(lab.devices().len(), 1);
}

#[test]
fn invalid_device_is_rejected() {
    let lab = Lab::new(Snapshot::default());
    assert!(matches!(
        lab.add_device("  ", DeviceKind::Sensor),
        Err(LabError::Invalid(_))
    ));
}

#[test]
fn set_state_emits_event_only_on_change() {
    let (lab, id) = lab_with("Lamp", DeviceKind::Switch);
    let mut rx = lab.subscribe();

    assert_eq!(
        lab.set_state(&id, true, Source::Ui).unwrap().state,
        State::Bool(true)
    );
    assert!(matches!(
        rx.try_recv().unwrap().kind,
        EventKind::StateChanged {
            state: State::Bool(true),
            ..
        }
    ));

    lab.set_state(&id, true, Source::Ui).unwrap();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn set_state_of_unknown_device_fails() {
    let lab = Lab::new(Snapshot::default());
    let id = DeviceId::parse("ghost").unwrap();
    assert!(matches!(
        lab.set_state(&id, true, Source::Ui),
        Err(LabError::NotFound { .. })
    ));
}

#[test]
fn remove_device_emits_event() {
    let (lab, id) = lab_with("Lamp", DeviceKind::Switch);
    let mut rx = lab.subscribe();

    lab.remove_device(&id).unwrap();

    assert!(lab.devices().is_empty());
    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::DeviceRemoved { id: id.clone() }
    );
    assert!(matches!(
        lab.remove_device(&id),
        Err(LabError::NotFound { .. })
    ));
}

#[test]
fn snapshot_channel_is_notified_on_every_change() {
    let (lab, id) = lab_with("Lamp", DeviceKind::Switch);
    let mut snapshots = lab.snapshots();
    snapshots.mark_unchanged();

    lab.set_state(&id, true, Source::Ui).unwrap();

    assert!(snapshots.has_changed().unwrap());
    assert_eq!(
        snapshots.borrow_and_update().snapshot.devices[0].state,
        State::Bool(true)
    );
}

#[test]
fn log_keeps_only_the_latest_events() {
    let (lab, id) = lab_with("Lamp", DeviceKind::Switch);
    for i in 0..LOG_CAPACITY + 10 {
        lab.set_state(&id, i % 2 == 0, Source::Ui).unwrap();
    }
    let log = lab.log();
    assert_eq!(log.len(), LOG_CAPACITY);
    assert!(matches!(
        log.last().unwrap().kind,
        EventKind::StateChanged {
            state: State::Bool(false),
            ..
        }
    ));
}

#[test]
fn restored_state_is_available() {
    let device = Device::new("Lamp", DeviceKind::Relay).unwrap();
    let scenario = Scenario::new("Demo", false, vec![set(&device.id, true)]).unwrap();
    let lab = Lab::new(Snapshot {
        devices: vec![device.clone()],
        scenarios: vec![scenario.clone()],
    });
    assert_eq!(lab.devices(), vec![device]);
    assert_eq!(lab.scenarios(), vec![scenario]);
}

#[test]
fn event_serializes_flat_with_type_tag() {
    let (lab, id) = lab_with("Lamp", DeviceKind::Switch);
    lab.set_state(&id, true, Source::Ui).unwrap();
    let json = serde_json::to_value(lab.log().last().unwrap()).unwrap();
    assert_eq!(json["type"], "state_changed");
    assert_eq!(json["id"], "lamp");
    assert_eq!(json["state"], true);
    assert_eq!(json["source"], serde_json::json!({ "type": "ui" }));
    assert!(json["at"].is_string());
}

#[test]
fn scenario_source_serializes_with_its_id() {
    let source = Source::Scenario {
        id: ScenarioId::parse("demo").unwrap(),
    };
    assert_eq!(
        serde_json::to_value(source).unwrap(),
        serde_json::json!({ "type": "scenario", "id": "demo" })
    );
}

#[test]
fn add_scenario_emits_event_and_updates_snapshot() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let mut rx = lab.subscribe();

    let scenario = lab
        .add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap();

    assert_eq!(lab.scenarios(), vec![scenario.clone()]);
    assert_eq!(lab.scenario(&scenario.id).unwrap(), scenario);
    assert_eq!(lab.snapshot().scenarios, vec![scenario.clone()]);
    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::ScenarioAdded { scenario }
    );
}

#[test]
fn duplicate_or_invalid_scenarios_are_rejected() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    lab.add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap();

    assert!(matches!(
        lab.add_scenario("demo", false, vec![set(&lamp, false)]),
        Err(LabError::AlreadyExists { .. })
    ));
    assert!(matches!(
        lab.add_scenario("Empty", false, vec![]),
        Err(LabError::Invalid(DomainError::NoSteps))
    ));
    assert_eq!(lab.scenarios().len(), 1);
}

#[test]
fn scenario_with_unknown_device_is_rejected() {
    let lab = Lab::new(Snapshot::default());
    let ghost = DeviceId::parse("ghost").unwrap();
    assert!(matches!(
        lab.add_scenario("Demo", false, vec![set(&ghost, true)]),
        Err(LabError::UnknownDevice(id)) if id == ghost
    ));
}

#[test]
fn update_scenario_keeps_the_id() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let id = lab
        .add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap()
        .id;
    let mut rx = lab.subscribe();

    let updated = lab
        .update_scenario(
            &id,
            "Renamed",
            true,
            vec![set(&lamp, true), Step::Delay { secs: 5 }],
        )
        .unwrap();

    assert_eq!(updated.id, id);
    assert_eq!(updated.name, "Renamed");
    assert!(updated.repeat);
    assert_eq!(lab.scenario(&id).unwrap(), updated);
    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::ScenarioUpdated { scenario: updated }
    );
}

#[test]
fn update_of_unknown_scenario_fails() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let id = ScenarioId::parse("ghost").unwrap();
    assert!(matches!(
        lab.update_scenario(&id, "x", false, vec![set(&lamp, true)]),
        Err(LabError::NotFound { .. })
    ));
}

#[test]
fn remove_scenario_emits_event() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let id = lab
        .add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap()
        .id;
    let mut rx = lab.subscribe();

    lab.remove_scenario(&id).unwrap();

    assert!(lab.scenarios().is_empty());
    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::ScenarioRemoved { id: id.clone() }
    );
    assert!(matches!(
        lab.remove_scenario(&id),
        Err(LabError::NotFound { .. })
    ));
}

#[test]
fn progress_events_are_broadcast_but_not_logged_or_persisted() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let id = lab
        .add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap()
        .id;
    let mut rx = lab.subscribe();
    let mut snapshots = lab.snapshots();
    snapshots.mark_unchanged();
    let logged = lab.log().len();

    lab.report(EventKind::ScenarioStep { id, step: 1 });

    assert!(matches!(
        rx.try_recv().unwrap().kind,
        EventKind::ScenarioStep { step: 1, .. }
    ));
    assert_eq!(lab.log().len(), logged);
    assert!(!snapshots.has_changed().unwrap());
}

#[test]
fn run_lifecycle_events_are_logged() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let id = lab
        .add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap()
        .id;
    let logged = lab.log().len();

    lab.report(EventKind::ScenarioStarted { id: id.clone() });
    lab.report(EventKind::ScenarioFinished { id });

    assert_eq!(lab.log().len(), logged + 2);
}

#[test]
fn mqtt_state_changes_are_announced_once() {
    let lab = Lab::new(Snapshot::default());
    let mut rx = lab.subscribe();
    assert_eq!(lab.mqtt_state(), MqttState::Disabled);

    lab.set_mqtt_state(MqttState::Connected);
    lab.set_mqtt_state(MqttState::Connected);

    assert_eq!(lab.mqtt_state(), MqttState::Connected);
    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::MqttStatus {
            state: MqttState::Connected
        }
    );
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(lab.log().len(), 1);
}

#[test]
fn single_device_lookup() {
    let (lab, id) = lab_with("Lamp", DeviceKind::Relay);
    assert_eq!(lab.device(&id).unwrap().kind, DeviceKind::Relay);
    let ghost = DeviceId::parse("ghost").unwrap();
    assert!(matches!(lab.device(&ghost), Err(LabError::NotFound { .. })));
}

#[test]
fn mqtt_source_serializes_with_a_type_tag() {
    assert_eq!(
        serde_json::to_value(Source::Mqtt).unwrap(),
        serde_json::json!({ "type": "mqtt" })
    );
}

#[test]
fn numeric_state_is_stored_and_announced() {
    let (lab, id) = lab_with("Humidity", DeviceKind::Percent);
    let mut rx = lab.subscribe();

    let device = lab.set_state(&id, 57, Source::Ui).unwrap();

    assert_eq!(device.state, State::Int(57));
    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::StateChanged {
            id: id.clone(),
            state: State::Int(57),
            source: Source::Ui
        }
    );
    lab.set_state(&id, 57, Source::Ui).unwrap();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn percent_rejects_values_outside_0_100() {
    let (lab, id) = lab_with("Humidity", DeviceKind::Percent);
    lab.set_state(&id, 40, Source::Ui).unwrap();

    for value in [-1, 101] {
        assert!(matches!(
            lab.set_state(&id, value, Source::Ui),
            Err(LabError::Invalid(DomainError::OutOfRange { .. }))
        ));
    }
    assert_eq!(lab.device(&id).unwrap().state, State::Int(40));
}

#[test]
fn temperature_accepts_negative_values() {
    let (lab, id) = lab_with("Outside", DeviceKind::Temperature);
    lab.set_state(&id, -12, Source::Ui).unwrap();
    assert_eq!(lab.device(&id).unwrap().state, State::Int(-12));
}

#[test]
fn state_type_must_match_the_device_kind() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let temp = lab
        .add_device("Outside", DeviceKind::Temperature)
        .unwrap()
        .id;

    assert!(matches!(
        lab.set_state(&lamp, 5, Source::Ui),
        Err(LabError::Invalid(DomainError::StateMismatch(_)))
    ));
    assert!(matches!(
        lab.set_state(&temp, true, Source::Ui),
        Err(LabError::Invalid(DomainError::StateMismatch(_)))
    ));
    assert_eq!(lab.device(&lamp).unwrap().state, State::Bool(false));
    assert_eq!(lab.device(&temp).unwrap().state, State::Int(0));
}

#[test]
fn scenario_steps_must_fit_the_device_kind() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let humidity = lab.add_device("Humidity", DeviceKind::Percent).unwrap().id;

    assert!(
        lab.add_scenario("Ok", false, vec![set(&lamp, true), set(&humidity, 80)])
            .is_ok()
    );
    assert!(matches!(
        lab.add_scenario("Number on a switch", false, vec![set(&lamp, 1)]),
        Err(LabError::Invalid(DomainError::StateMismatch(_)))
    ));
    assert!(matches!(
        lab.add_scenario("Too humid", false, vec![set(&humidity, 150)]),
        Err(LabError::Invalid(DomainError::OutOfRange { .. }))
    ));
    assert!(matches!(
        lab.add_scenario("On/off on a number", false, vec![set(&humidity, true)]),
        Err(LabError::Invalid(DomainError::StateMismatch(_)))
    ));
}

#[test]
fn updating_a_scenario_checks_its_steps_too() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let id = lab
        .add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap()
        .id;
    assert!(matches!(
        lab.update_scenario(&id, "Demo", false, vec![set(&lamp, 7)]),
        Err(LabError::Invalid(DomainError::StateMismatch(_)))
    ));
    assert_eq!(lab.scenario(&id).unwrap().steps, vec![set(&lamp, true)]);
}

#[test]
fn devices_go_offline_and_back_with_events() {
    let (lab, id) = lab_with("Door", DeviceKind::Sensor);
    let mut rx = lab.subscribe();

    assert!(!lab.set_online(&id, false, Source::Ui).unwrap().online);
    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::AvailabilityChanged {
            id: id.clone(),
            online: false,
            source: Source::Ui
        }
    );
    assert!(!lab.device(&id).unwrap().online);

    lab.set_online(&id, false, Source::Ui).unwrap();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

    assert!(lab.set_online(&id, true, Source::Ui).unwrap().online);
    assert!(matches!(
        rx.try_recv().unwrap().kind,
        EventKind::AvailabilityChanged { online: true, .. }
    ));
}

#[test]
fn availability_of_an_unknown_device_fails() {
    let lab = Lab::new(Snapshot::default());
    let ghost = DeviceId::parse("ghost").unwrap();
    assert!(matches!(
        lab.set_online(&ghost, false, Source::Ui),
        Err(LabError::NotFound { .. })
    ));
}

#[test]
fn availability_is_persisted_and_logged() {
    let (lab, id) = lab_with("Door", DeviceKind::Sensor);
    lab.set_online(&id, false, Source::Ui).unwrap();

    assert!(!lab.snapshot().devices[0].online);
    assert!(matches!(
        lab.log().last().unwrap().kind,
        EventKind::AvailabilityChanged { online: false, .. }
    ));
}

#[test]
fn an_offline_device_can_still_change_state() {
    let (lab, id) = lab_with("Outside", DeviceKind::Temperature);
    lab.set_online(&id, false, Source::Ui).unwrap();

    lab.set_state(&id, 7, Source::Ui).unwrap();

    let device = lab.device(&id).unwrap();
    assert_eq!(device.state, State::Int(7));
    assert!(!device.online);
}

#[test]
fn availability_steps_need_an_existing_device() {
    let (lab, door) = lab_with("Door", DeviceKind::Sensor);
    let step = |device: &DeviceId| Step::Availability {
        device: device.clone(),
        online: false,
    };

    assert!(lab.add_scenario("Drop", false, vec![step(&door)]).is_ok());
    let ghost = DeviceId::parse("ghost").unwrap();
    assert!(matches!(
        lab.add_scenario("Ghost", false, vec![step(&ghost)]),
        Err(LabError::UnknownDevice(id)) if id == ghost
    ));
}

#[test]
fn a_device_used_by_scenarios_cannot_be_removed() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let door = lab.add_device("Door", DeviceKind::Sensor).unwrap().id;
    lab.add_scenario("Night", false, vec![set(&lamp, true)])
        .unwrap();
    lab.add_scenario("Alarm", false, vec![set(&lamp, false), set(&door, true)])
        .unwrap();
    let mut rx = lab.subscribe();

    let error = lab.remove_device(&lamp).unwrap_err();

    assert!(matches!(
        &error,
        LabError::DeviceInUse { device, scenarios }
            if device == &lamp && scenarios.len() == 2
    ));
    assert_eq!(
        error.to_string(),
        "device lamp is used by scenarios night, alarm; remove it from the scenario first"
    );
    assert!(lab.device(&lamp).is_ok());
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn a_device_can_be_removed_once_no_scenario_uses_it() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let night = lab
        .add_scenario("Night", false, vec![set(&lamp, true)])
        .unwrap()
        .id;

    assert!(lab.remove_device(&lamp).is_err());
    lab.remove_scenario(&night).unwrap();
    assert!(lab.remove_device(&lamp).is_ok());
}

#[test]
fn availability_steps_also_keep_a_device_in_use() {
    let (lab, door) = lab_with("Door", DeviceKind::Sensor);
    lab.add_scenario(
        "Drop",
        false,
        vec![Step::Availability {
            device: door.clone(),
            online: false,
        }],
    )
    .unwrap();
    assert!(matches!(
        lab.remove_device(&door),
        Err(LabError::DeviceInUse { .. })
    ));
}

#[test]
fn renaming_a_scenario_to_a_taken_name_is_rejected() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    let steps = || vec![set(&lamp, true)];
    let night = lab.add_scenario("Night", false, steps()).unwrap().id;
    lab.add_scenario("Beta", false, steps()).unwrap();

    assert!(matches!(
        lab.update_scenario(&night, "beta", false, steps()),
        Err(LabError::AlreadyExists { .. })
    ));
    assert_eq!(lab.scenario(&night).unwrap().name, "Night");
    // Changing only the case or spacing of its own name is fine.
    assert!(
        lab.update_scenario(&night, "NIGHT  ", false, steps())
            .is_ok()
    );
}

#[test]
fn a_panic_while_the_lock_is_held_does_not_poison_the_lab() {
    let (lab, id) = lab_with("Lamp", DeviceKind::Switch);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = lab.lock();
        panic!("simulated failure while holding the lock");
    }));
    assert!(result.is_err());

    assert_eq!(lab.devices().len(), 1);
    assert!(lab.set_state(&id, true, Source::Ui).is_ok());
}

#[test]
fn updating_a_missing_scenario_is_not_found_even_if_its_name_is_taken() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    lab.add_scenario("Night", false, vec![set(&lamp, true)])
        .unwrap();
    let missing = ScenarioId::parse("ghost").unwrap();

    assert!(matches!(
        lab.update_scenario(&missing, "Night", false, vec![set(&lamp, true)]),
        Err(LabError::NotFound { .. })
    ));
}

fn project(name: &str) -> ProjectId {
    ProjectId::parse(name).unwrap()
}

fn snapshot_of(name: &str, kind: DeviceKind) -> Snapshot {
    Snapshot {
        devices: vec![Device::new(name, kind).unwrap()],
        ..Snapshot::default()
    }
}

#[test]
fn a_lab_starts_in_the_default_project_unless_told_otherwise() {
    assert_eq!(
        Lab::new(Snapshot::default()).project(),
        ProjectId::default_project()
    );
    let lab = Lab::with_project(project("home"), Snapshot::default());
    assert_eq!(lab.project(), project("home"));
    assert_eq!(lab.snapshots().borrow().project, project("home"));
}

#[test]
fn replace_swaps_everything_and_returns_the_previous_state() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    lab.set_state(&lamp, true, Source::Ui).unwrap();
    lab.add_scenario("Night", false, vec![set(&lamp, false)])
        .unwrap();
    let before = lab.snapshot();

    let (old_project, old_snapshot) =
        lab.replace(project("dacha"), snapshot_of("Gate", DeviceKind::Sensor));

    assert_eq!(old_project, ProjectId::default_project());
    assert_eq!(
        old_snapshot, before,
        "the caller gets the last state of the old project"
    );
    assert_eq!(lab.project(), project("dacha"));
    let devices = lab.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id.as_str(), "gate");
    assert!(lab.scenarios().is_empty());
    assert!(lab.device(&lamp).is_err());
}

#[test]
fn replace_starts_a_fresh_log_with_the_switch_as_its_first_entry() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    lab.set_state(&lamp, true, Source::Ui).unwrap();
    assert!(lab.log().len() >= 2);

    lab.replace(project("dacha"), Snapshot::default());

    let log = lab.log();
    assert_eq!(log.len(), 1);
    assert_eq!(
        log[0].kind,
        EventKind::ProjectChanged {
            project: project("dacha")
        }
    );
}

#[test]
fn replace_announces_the_switch_and_publishes_the_new_project_for_persistence() {
    let (lab, _) = lab_with("Lamp", DeviceKind::Switch);
    let mut rx = lab.subscribe();
    let mut persisted = lab.snapshots();
    persisted.mark_unchanged();

    lab.replace(project("dacha"), snapshot_of("Gate", DeviceKind::Sensor));

    assert_eq!(
        rx.try_recv().unwrap().kind,
        EventKind::ProjectChanged {
            project: project("dacha")
        }
    );
    assert!(persisted.has_changed().unwrap());
    let payload = persisted.borrow_and_update().clone();
    assert_eq!(payload.project, project("dacha"));
    assert_eq!(payload.snapshot.devices[0].id.as_str(), "gate");
}

#[test]
fn changes_after_a_switch_are_persisted_under_the_new_project() {
    let (lab, _) = lab_with("Lamp", DeviceKind::Switch);
    lab.replace(project("dacha"), snapshot_of("Gate", DeviceKind::Sensor));
    let gate = DeviceId::parse("gate").unwrap();

    lab.set_state(&gate, true, Source::Ui).unwrap();

    let payload = lab.snapshots().borrow().clone();
    assert_eq!(payload.project, project("dacha"));
    assert_eq!(payload.snapshot.devices[0].state, State::Bool(true));
}

fn full_of_devices() -> Lab {
    let devices = (0..MAX_DEVICES)
        .map(|i| Device::new(&format!("Device {i}"), DeviceKind::Sensor).unwrap())
        .collect();
    Lab::new(Snapshot {
        devices,
        ..Snapshot::default()
    })
}

#[test]
fn a_project_cannot_grow_past_the_device_limit() {
    let lab = full_of_devices();
    let mut rx = lab.subscribe();

    let error = lab.add_device("One more", DeviceKind::Switch).unwrap_err();

    assert!(
        matches!(
            error,
            LabError::LimitReached {
                what: "devices",
                max: MAX_DEVICES
            }
        ),
        "{error}"
    );
    assert_eq!(lab.devices().len(), MAX_DEVICES);
    assert!(rx.try_recv().is_err(), "a refused device is not announced");

    lab.remove_device(&DeviceId::parse("device-0").unwrap())
        .unwrap();
    lab.add_device("One more", DeviceKind::Switch).unwrap();
}

#[test]
fn a_project_cannot_grow_past_the_scenario_limit() {
    let (lab, lamp) = lab_with("Lamp", DeviceKind::Switch);
    for i in 0..MAX_SCENARIOS {
        lab.add_scenario(&format!("Scenario {i}"), false, vec![set(&lamp, true)])
            .unwrap();
    }

    let error = lab
        .add_scenario("One more", false, vec![set(&lamp, true)])
        .unwrap_err();

    assert!(
        matches!(
            error,
            LabError::LimitReached {
                what: "scenarios",
                max: MAX_SCENARIOS
            }
        ),
        "{error}"
    );
    assert_eq!(lab.scenarios().len(), MAX_SCENARIOS);
}

#[test]
fn the_project_list_event_is_not_logged() {
    let lab = Lab::new(Snapshot::default());
    let mut rx = lab.subscribe();

    lab.report(EventKind::ProjectsChanged);

    assert_eq!(rx.try_recv().unwrap().kind, EventKind::ProjectsChanged);
    assert!(lab.log().is_empty());
}
