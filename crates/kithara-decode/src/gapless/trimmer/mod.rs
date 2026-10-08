//! Gapless trimmer — applies leading/trailing trim to a decoded PCM stream.

mod buffer;
mod core;
mod fade;
mod heuristic;
mod silence;
#[cfg(test)]
mod tests;

pub use core::{GaplessOutput, GaplessTrimmer};
