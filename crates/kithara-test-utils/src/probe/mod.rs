#[cfg(not(feature = "usdt"))]
mod noop;
#[cfg(feature = "usdt")]
mod real;
mod traits;

#[cfg(not(feature = "usdt"))]
pub use noop::*;
#[cfg(feature = "usdt")]
pub use real::*;
pub use traits::{IntoProbeArg, Probe, operation_id};
