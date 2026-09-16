pub use core::time::Duration;

pub(crate) use web_time::Instant;
pub use web_time::{Instant as WallInstant, SystemTime};

/// Error returned when an async operation exceeds its deadline.
#[derive(Debug, derive_more::Display)]
#[display("operation timed out")]
#[derive(derive_more::Error)]
#[error(ignore)]
pub struct TimeoutError;
