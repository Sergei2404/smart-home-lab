use super::*;
use crate::domain::{DeviceId, ScenarioId, Step};
use std::collections::HashSet;

#[test]
fn preset_ids_are_unique_and_documented() {
    let ids: HashSet<_> = PRESETS.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), PRESETS.len());
    for preset in &PRESETS {
        assert!(
            !preset.title.is_empty() && !preset.description.is_empty(),
            "{}",
            preset.id
        );
        assert_eq!(find(preset.id).map(|p| p.id), Some(preset.id));
    }
    assert!(find("nope").is_none());
}

#[test]
fn every_preset_passes_the_rules_the_api_enforces() {
    for preset in &PRESETS {
        let snapshot = preset.snapshot();
        snapshot
            .validate()
            .unwrap_or_else(|e| panic!("preset {} is invalid: {e}", preset.id));
        assert!(snapshot.devices.len() >= 8, "{} is too small", preset.id);
        assert!(
            snapshot.scenarios.len() >= 3,
            "{} has too few scenarios",
            preset.id
        );
    }
}

#[test]
fn ids_follow_the_names_and_scenarios_only_use_existing_devices() {
    for preset in &PRESETS {
        let snapshot = preset.snapshot();
        let devices: HashSet<&DeviceId> = snapshot.devices.iter().map(|d| &d.id).collect();
        for device in &snapshot.devices {
            assert_eq!(
                DeviceId::from_name(&device.name).as_ref(),
                Ok(&device.id),
                "{}",
                preset.id
            );
        }
        for scenario in &snapshot.scenarios {
            assert_eq!(
                ScenarioId::from_name(&scenario.name).as_ref(),
                Ok(&scenario.id)
            );
            for step in &scenario.steps {
                if let Step::Set { device, .. } | Step::Availability { device, .. } = step {
                    assert!(
                        devices.contains(device),
                        "{}: scenario {} uses unknown device {device}",
                        preset.id,
                        scenario.id
                    );
                }
            }
        }
    }
}

#[test]
fn presets_are_ready_to_use() {
    for preset in &PRESETS {
        let snapshot = preset.snapshot();
        assert!(snapshot.devices.iter().all(|d| d.online), "{}", preset.id);
        assert!(
            snapshot.scenarios.iter().all(|s| !s.repeat),
            "{}",
            preset.id
        );
    }
}
