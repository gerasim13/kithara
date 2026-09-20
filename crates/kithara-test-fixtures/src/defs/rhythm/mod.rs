#![cfg(feature = "rhythm")]

mod analyze;
mod assets;
mod score;

#[cfg(feature = "library")]
pub(super) use analyze::beat_flac;
#[cfg(feature = "library")]
pub(super) use assets::analysis_file;
