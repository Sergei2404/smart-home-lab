use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, Unexpected, Visitor},
};
use std::fmt;

/// The state of a device: on/off for switches and binary sensors, a whole number for numeric sensors.
/// In JSON it is a plain `true` / `false` or an integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum State {
    Bool(bool),
    Int(i32),
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StateVisitor)
    }
}

/// Hand-written so that a bad value says what is expected, not "did not match any variant".
struct StateVisitor;

impl Visitor<'_> for StateVisitor {
    type Value = State;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("true, false or a whole number between -2147483648 and 2147483647")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<State, E> {
        Ok(State::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<State, E> {
        i32::try_from(value)
            .map(State::Int)
            .map_err(|_| E::invalid_value(Unexpected::Signed(value), &self))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<State, E> {
        i32::try_from(value)
            .map(State::Int)
            .map_err(|_| E::invalid_value(Unexpected::Unsigned(value), &self))
    }
}

impl From<bool> for State {
    fn from(on: bool) -> Self {
        Self::Bool(on)
    }
}

impl From<i32> for State {
    fn from(value: i32) -> Self {
        Self::Int(value)
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(true) => f.write_str("on"),
            Self::Bool(false) => f.write_str("off"),
            Self::Int(value) => write!(f, "{value}"),
        }
    }
}

#[cfg(test)]
mod tests;
