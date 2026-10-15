use super::*;

fn set(device: &str, state: impl Into<State>) -> Step {
    Step::Set {
        device: DeviceId::parse(device).unwrap(),
        state: state.into(),
    }
}

fn delay(secs: u64) -> Step {
    Step::Delay { secs }
}

#[test]
fn valid_scenario_gets_an_id_from_its_name() {
    let scenario = Scenario::new(
        " Night Light ",
        false,
        vec![set("lamp", true), delay(10), set("lamp", false)],
    )
    .unwrap();
    assert_eq!(scenario.id.as_str(), "night-light");
    assert_eq!(scenario.name, "Night Light");
    assert_eq!(scenario.devices().count(), 2);
}

#[test]
fn validate_applies_the_same_rules_as_new() {
    let mut scenario = Scenario::new("Blink", true, vec![set("lamp", true), delay(1)]).unwrap();
    assert!(scenario.validate().is_ok());

    scenario.steps = vec![set("lamp", true)];
    assert_eq!(scenario.validate(), Err(DomainError::LoopWithoutDelay));
    scenario.repeat = false;
    scenario.steps = vec![delay(0)];
    assert_eq!(scenario.validate(), Err(DomainError::InvalidDelay(0)));
    scenario.steps.clear();
    assert_eq!(scenario.validate(), Err(DomainError::NoSteps));
}

#[test]
fn scenario_without_steps_is_rejected() {
    assert_eq!(Scenario::new("x", false, vec![]), Err(DomainError::NoSteps));
}

#[test]
fn too_many_steps_are_rejected() {
    let steps = vec![set("lamp", true); MAX_STEPS + 1];
    assert_eq!(
        Scenario::new("x", false, steps),
        Err(DomainError::TooManySteps)
    );
}

#[test]
fn delay_must_be_within_bounds() {
    for secs in [0, MAX_DELAY_SECS + 1] {
        assert_eq!(
            Scenario::new("x", false, vec![delay(secs)]),
            Err(DomainError::InvalidDelay(secs))
        );
    }
    for secs in [1, MAX_DELAY_SECS] {
        assert!(Scenario::new("x", false, vec![delay(secs)]).is_ok());
    }
}

#[test]
fn repeating_scenario_needs_a_delay() {
    assert_eq!(
        Scenario::new("x", true, vec![set("lamp", true), set("lamp", false)]),
        Err(DomainError::LoopWithoutDelay)
    );
    assert!(Scenario::new("x", true, vec![set("lamp", true), delay(1)]).is_ok());
    assert!(Scenario::new("x", false, vec![set("lamp", true)]).is_ok());
}

#[test]
fn invalid_name_is_rejected() {
    assert_eq!(
        Scenario::new("  ", false, vec![delay(1)]),
        Err(DomainError::InvalidName)
    );
    assert_eq!(
        Scenario::new("Сценарій", false, vec![delay(1)]),
        Err(DomainError::EmptySlug)
    );
}

#[test]
fn steps_use_a_type_tag() {
    let steps = vec![set("lamp", true), set("temp", -5), delay(10)];
    let json = serde_json::to_value(&steps).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            { "type": "set", "device": "lamp", "state": true },
            { "type": "set", "device": "temp", "state": -5 },
            { "type": "delay", "secs": 10 }
        ])
    );
    let parsed: Vec<Step> = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, steps);
}

#[test]
fn availability_steps_round_trip() {
    let step = Step::Availability {
        device: DeviceId::parse("door").unwrap(),
        online: false,
    };
    let json = serde_json::to_value(&step).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "type": "availability", "device": "door", "online": false })
    );
    assert_eq!(serde_json::from_value::<Step>(json).unwrap(), step);
}

#[test]
fn availability_steps_count_as_device_references_but_not_as_delays() {
    let steps = vec![Step::Availability {
        device: DeviceId::parse("door").unwrap(),
        online: false,
    }];
    let scenario = Scenario::new("Drop", false, steps.clone()).unwrap();
    assert_eq!(scenario.devices().count(), 1);
    assert_eq!(
        Scenario::new("Drop", true, steps),
        Err(DomainError::LoopWithoutDelay)
    );
}

#[test]
fn unknown_step_type_is_rejected() {
    assert!(serde_json::from_str::<Step>(r#"{"type":"explode"}"#).is_err());
}
