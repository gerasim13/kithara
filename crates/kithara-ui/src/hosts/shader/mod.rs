//! Document shader snapshots and toolkit-specific GPU adapters.

mod frame;
#[cfg(test)]
mod tests;

#[cfg(feature = "masonry")]
pub(crate) use frame::ShaderFrameError;
pub(crate) use frame::{ShaderFrame, logical_extent};
