use super::{MAX_DELAY_SECS, MAX_STEPS};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("name must be 1-64 characters without control characters")]
    InvalidName,
    #[error("name must contain at least one latin letter or digit")]
    EmptySlug,
    #[error("invalid id {0:?}: use 1-32 characters of a-z, 0-9, '-' and '_'")]
    InvalidId(String),
    #[error("a scenario needs at least one step")]
    NoSteps,
    #[error("a scenario can have at most {max} steps", max = MAX_STEPS)]
    TooManySteps,
    #[error("a delay must be between 1 and {max} seconds, got {0}", max = MAX_DELAY_SECS)]
    InvalidDelay(u64),
    #[error("a repeating scenario needs at least one delay step")]
    LoopWithoutDelay,
    #[error("this device needs {0}")]
    StateMismatch(&'static str),
    #[error("value {value} is outside the range {min}..={max}")]
    OutOfRange { value: i32, min: i32, max: i32 },
}
