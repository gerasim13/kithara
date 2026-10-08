//! Visualiser reads, uniform packing, and toolkit-specific GPU adapters.

#[cfg(all(test, any(feature = "gpu", feature = "masonry")))]
pub(crate) mod fixture;
mod frame;
mod uniform;

pub(crate) use frame::VisFrame;
pub(crate) use uniform::{Uniforms, consts::SHADER};
