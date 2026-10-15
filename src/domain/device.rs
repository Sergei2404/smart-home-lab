use super::{
    DomainError, State,
    naming::{slug_id, validate_name},
};
use serde::{Deserialize, Serialize};

pub const PERCENT_MIN: i32 = 0;
pub const PERCENT_MAX: i32 = 100;

slug_id!(
    /// Stable, topic-safe identifier derived from the device name (`"Living Lamp"` -> `living-lamp`).
    DeviceId
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// On/off sensor (motion, door contact, ...).
    Sensor,
    Switch,
    Relay,
    /// Whole number from 0 to 100 (humidity, battery level, ...).
    Percent,
    /// Whole number of degrees Celsius, negative values allowed.
    Temperature,
}

impl DeviceKind {
    pub fn initial_state(self) -> State {
        if self.is_numeric() {
            State::Int(0)
        } else {
            State::Bool(false)
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, Self::Percent | Self::Temperature)
    }

    /// Only switches and relays can be controlled from outside (Home Assistant).
    pub fn accepts_commands(self) -> bool {
        matches!(self, Self::Switch | Self::Relay)
    }

    /// Checks that `state` has the right type for this kind and is within its range.
    pub fn check(self, state: State) -> Result<State, DomainError> {
        match (self, state) {
            (Self::Sensor | Self::Switch | Self::Relay, State::Bool(_)) => Ok(state),
            (Self::Sensor | Self::Switch | Self::Relay, State::Int(_)) => {
                Err(DomainError::StateMismatch("an on/off state"))
            }
            (Self::Percent | Self::Temperature, State::Bool(_)) => {
                Err(DomainError::StateMismatch("a whole number"))
            }
            (Self::Percent, State::Int(value)) if !(PERCENT_MIN..=PERCENT_MAX).contains(&value) => {
                Err(DomainError::OutOfRange {
                    value,
                    min: PERCENT_MIN,
                    max: PERCENT_MAX,
                })
            }
            (Self::Percent | Self::Temperature, State::Int(_)) => Ok(state),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub kind: DeviceKind,
    pub state: State,
    /// Offline devices keep their state but stop reporting. Optional in a file: defaults to online.
    #[serde(default = "online_by_default")]
    pub online: bool,
}

fn online_by_default() -> bool {
    true
}

impl Device {
    /// Re-checks a device that did not come from `new` (for example one read from disk).
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_name(&self.name)?;
        self.kind.check(self.state).map(|_| ())
    }

    pub fn new(name: &str, kind: DeviceKind) -> Result<Self, DomainError> {
        let name = validate_name(name)?;
        Ok(Self {
            id: DeviceId::from_name(name)?,
            name: name.to_owned(),
            kind,
            state: kind.initial_state(),
            online: true,
        })
    }
}

#[cfg(test)]
mod tests;
