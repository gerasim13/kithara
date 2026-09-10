mod config;
mod core;
#[cfg(any(test, feature = "probe"))]
mod mix;
mod slots;

pub use core::EngineImpl;

pub use config::{DEFAULT_GATE_SMOOTHING, EngineConfig};
#[cfg(any(test, feature = "probe"))]
pub use mix::apply_mix;
