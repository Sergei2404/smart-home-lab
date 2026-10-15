use super::{
    DeviceId, DomainError, State,
    naming::{slug_id, validate_name},
};
use serde::{Deserialize, Serialize};

pub const MAX_STEPS: usize = 100;
pub const MAX_DELAY_SECS: u64 = 86_400;

slug_id!(
    /// Stable identifier derived from the scenario name.
    ScenarioId
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
    Set {
        device: DeviceId,
        state: State,
    },
    /// Takes a device off the network or brings it back.
    Availability {
        device: DeviceId,
        online: bool,
    },
    Delay {
        secs: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub id: ScenarioId,
    pub name: String,
    pub repeat: bool,
    pub steps: Vec<Step>,
}

impl Scenario {
    pub fn new(name: &str, repeat: bool, steps: Vec<Step>) -> Result<Self, DomainError> {
        let name = validate_name(name)?;
        Self::with_id(ScenarioId::from_name(name)?, name, repeat, steps)
    }

    pub fn with_id(
        id: ScenarioId,
        name: &str,
        repeat: bool,
        steps: Vec<Step>,
    ) -> Result<Self, DomainError> {
        let name = validate_name(name)?;
        validate_steps(&steps, repeat)?;
        Ok(Self {
            id,
            name: name.to_owned(),
            repeat,
            steps,
        })
    }

    /// Re-checks a scenario that did not come from `new` (for example one read from disk).
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_name(&self.name)?;
        validate_steps(&self.steps, self.repeat)
    }

    /// Devices referenced by the steps, in step order (a device may repeat).
    pub fn devices(&self) -> impl Iterator<Item = &DeviceId> {
        self.steps.iter().filter_map(|step| match step {
            Step::Set { device, .. } | Step::Availability { device, .. } => Some(device),
            Step::Delay { .. } => None,
        })
    }
}

fn validate_steps(steps: &[Step], repeat: bool) -> Result<(), DomainError> {
    if steps.is_empty() {
        return Err(DomainError::NoSteps);
    }
    if steps.len() > MAX_STEPS {
        return Err(DomainError::TooManySteps);
    }
    for step in steps {
        if let Step::Delay { secs } = step
            && !(1..=MAX_DELAY_SECS).contains(secs)
        {
            return Err(DomainError::InvalidDelay(*secs));
        }
    }
    // Without a delay a repeating scenario would spin the CPU forever.
    if repeat && !steps.iter().any(|s| matches!(s, Step::Delay { .. })) {
        return Err(DomainError::LoopWithoutDelay);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
