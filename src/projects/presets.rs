use crate::domain::Snapshot;
use serde::Serialize;
use std::sync::LazyLock;

/// A ready-made starting point for a new project. The data is embedded in the binary.
pub struct Preset {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    json: &'static str,
}

/// What the picker shows about a preset.
#[derive(Debug, Clone, Serialize)]
pub struct PresetInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub devices: usize,
    pub scenarios: usize,
}

pub static PRESETS: [Preset; 2] = [
    Preset {
        id: "apartment",
        title: "Apartment",
        description: "Lamps, relays, motion and door sensors, a thermometer and a humidity sensor, with four scenarios.",
        json: include_str!("../../presets/apartment.json"),
    },
    Preset {
        id: "dacha",
        title: "Dacha",
        description: "A country house: gate, pump, boiler, temperatures, soil moisture and a battery level, with five scenarios.",
        json: include_str!("../../presets/dacha.json"),
    },
];

impl Preset {
    pub fn snapshot(&self) -> Snapshot {
        serde_json::from_str(self.json).expect("built-in preset is valid (a unit test checks it)")
    }

    fn info(&self) -> PresetInfo {
        let snapshot = self.snapshot();
        PresetInfo {
            id: self.id,
            title: self.title,
            description: self.description,
            devices: snapshot.devices.len(),
            scenarios: snapshot.scenarios.len(),
        }
    }
}

/// What the picker shows about every preset (parsed once, not on every request).
pub fn infos() -> &'static [PresetInfo] {
    static INFOS: LazyLock<Vec<PresetInfo>> =
        LazyLock::new(|| PRESETS.iter().map(Preset::info).collect());
    &INFOS
}

pub fn find(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

#[cfg(test)]
mod tests;
