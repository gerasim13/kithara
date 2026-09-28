//! Gapless trimmer — applies leading/trailing trim to a decoded PCM stream.

mod core;
#[cfg(test)]
mod tests;

pub use core::{GaplessOutput, GaplessTrimmer};
