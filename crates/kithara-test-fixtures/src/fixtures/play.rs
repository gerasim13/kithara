use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared constant PCM for playback source contracts.
#[kithara::fixture]
#[must_use]
pub fn half() -> Vec<f32> {
    samples(&assets::play_input_half())
}

/// Prepared constant PCM for playback source contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter() -> Vec<f32> {
    samples(&assets::play_input_quarter())
}

/// Prepared constant PCM for playback source contracts.
#[kithara::fixture]
#[must_use]
pub fn negative_quarter() -> Vec<f32> {
    samples(&assets::play_input_negative_quarter())
}

/// Prepared constant PCM for playback source contracts.
#[kithara::fixture]
#[must_use]
pub fn negative_half() -> Vec<f32> {
    samples(&assets::play_input_negative_half())
}

/// Prepared constant PCM for playback source contracts.
#[kithara::fixture]
#[must_use]
pub fn three_quarter() -> Vec<f32> {
    samples(&assets::play_input_three_quarter())
}

/// Prepared PCM for recording into application assets.
#[kithara::fixture]
#[must_use]
pub fn recording() -> Vec<f32> {
    samples(&assets::play_input_recording())
}
