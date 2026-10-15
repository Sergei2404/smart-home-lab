use super::{Device, DeviceId, DomainError, Scenario, ScenarioId, Step};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The most devices and scenarios one project may hold: a bound on what a file or request can make it
/// grow to. A project at both limits still fits in one request body.
pub const MAX_DEVICES: usize = 500;
pub const MAX_SCENARIOS: usize = 100;

/// Everything that is persisted between restarts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub scenarios: Vec<Scenario>,
}

/// A snapshot read from disk that breaks the rules the API enforces.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("device {id}: {reason}")]
    Device { id: DeviceId, reason: DomainError },
    #[error("device id {0} appears more than once")]
    DuplicateDevice(DeviceId),
    #[error("scenario {id}: {reason}")]
    Scenario { id: ScenarioId, reason: DomainError },
    #[error("scenario id {0} appears more than once")]
    DuplicateScenario(ScenarioId),
    #[error("a project can have at most {max} {what}, this one has {found}")]
    TooMany {
        what: &'static str,
        max: usize,
        found: usize,
    },
}

impl Snapshot {
    /// Applies the rules the API applies to a request, so a hand-edited file cannot smuggle in what the
    /// API would refuse (a repeating scenario without a delay would spin the CPU). A step naming a device
    /// that no longer exists is accepted; running it fails with a clear message.
    pub fn validate(&self) -> Result<(), SnapshotError> {
        for (what, max, found) in [
            ("devices", MAX_DEVICES, self.devices.len()),
            ("scenarios", MAX_SCENARIOS, self.scenarios.len()),
        ] {
            if found > max {
                return Err(SnapshotError::TooMany { what, max, found });
            }
        }
        let mut seen = HashSet::new();
        for device in &self.devices {
            if !seen.insert(&device.id) {
                return Err(SnapshotError::DuplicateDevice(device.id.clone()));
            }
            device.validate().map_err(|reason| SnapshotError::Device {
                id: device.id.clone(),
                reason,
            })?;
        }

        let mut seen = HashSet::new();
        for scenario in &self.scenarios {
            let fail = |reason| SnapshotError::Scenario {
                id: scenario.id.clone(),
                reason,
            };
            if !seen.insert(&scenario.id) {
                return Err(SnapshotError::DuplicateScenario(scenario.id.clone()));
            }
            scenario.validate().map_err(fail)?;
            for step in &scenario.steps {
                if let Step::Set { device, state } = step
                    && let Some(known) = self.devices.iter().find(|d| &d.id == device)
                {
                    known.kind.check(*state).map_err(fail)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
