//! Shared audio renderer — cooperative multi-track scheduler on a dedicated OS thread.

mod gate;
mod load;
mod node;
mod source;
#[cfg(test)]
mod tests;

pub use gate::PreloadGate;
pub use load::{EngineLoad, EngineLoadSnapshot};
pub(crate) use node::{DecoderNode, TrackRegistration};
pub(crate) use source::{apply_effects, reset_effects};
#[cfg(test)]
pub(crate) use tests::MockSource;

pub use crate::runtime::ServiceClass;
pub(crate) use crate::runtime::{observer::HangWatchdogObserver, wake::ThreadWake};
