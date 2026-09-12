mod config;
mod core;
#[cfg(test)]
mod mix;
mod slots;

pub use core::EngineImpl;

pub use config::{DEFAULT_GATE_SMOOTHING, EngineConfig};
#[cfg(test)]
pub use mix::apply_mix;
