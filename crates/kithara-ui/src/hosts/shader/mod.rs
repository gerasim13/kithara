//! Document shader snapshots and toolkit-specific GPU adapters.

pub(crate) mod frame;
#[cfg(test)]
mod tests;

pub(crate) use frame::{ShaderFrame, logical_extent};
