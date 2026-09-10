use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn run_ramp() -> Vec<f32> {
    samples(&assets::beat_input_run_ramp())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn fragments() -> Vec<f32> {
    samples(&assets::beat_input_fragments())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn sine_440() -> Vec<f32> {
    samples(&assets::beat_input_sine_440())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn sine_220() -> Vec<f32> {
    samples(&assets::beat_input_sine_220())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn nn_tone() -> Vec<f32> {
    samples(&assets::beat_input_nn_tone())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn step() -> Vec<f32> {
    samples(&assets::beat_input_step())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn cancelling() -> Vec<f32> {
    samples(&assets::beat_input_cancelling())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter_4096() -> Vec<f32> {
    samples(&assets::beat_input_quarter_4096())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter_10000() -> Vec<f32> {
    samples(&assets::beat_input_quarter_10000())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter_44100() -> Vec<f32> {
    samples(&assets::beat_input_quarter_44100())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter_88200() -> Vec<f32> {
    samples(&assets::beat_input_quarter_88200())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter_132300() -> Vec<f32> {
    samples(&assets::beat_input_quarter_132300())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter_176400() -> Vec<f32> {
    samples(&assets::beat_input_quarter_176400())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter_529200() -> Vec<f32> {
    samples(&assets::beat_input_quarter_529200())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn quarter_2646000() -> Vec<f32> {
    samples(&assets::beat_input_quarter_2646000())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn tenth_4096() -> Vec<f32> {
    samples(&assets::beat_input_tenth_4096())
}

/// Prepared PCM for beat analysis contracts.
#[kithara::fixture]
#[must_use]
pub fn tenth_816000() -> Vec<f32> {
    samples(&assets::beat_input_tenth_816000())
}

/// Prepared stereo PCM for analysis archive resume contracts.
#[kithara::fixture]
#[must_use]
pub fn archive_tone() -> Vec<f32> {
    samples(&assets::beat_input_archive_tone())
}

/// Prepared PCM for the analysis producer contract.
#[kithara::fixture]
#[must_use]
pub fn producer_stereo() -> Vec<f32> {
    samples(&assets::beat_input_producer_stereo())
}

/// Prepared PCM for the analysis producer contract.
#[kithara::fixture]
#[must_use]
pub fn producer_unity() -> Vec<f32> {
    samples(&assets::beat_input_producer_unity())
}

/// Prepared PCM for the analysis producer contract.
#[kithara::fixture]
#[must_use]
pub fn producer_mono() -> Vec<f32> {
    samples(&assets::beat_input_producer_mono())
}

/// Prepared phase-locked PCM for fused gapless seams.
#[kithara::fixture]
#[must_use]
pub fn fused_seam() -> Vec<f32> {
    samples(&assets::beat_input_fused_seam())
}

/// Prepared phase-locked PCM for fused gapless seams.
#[kithara::fixture]
#[must_use]
pub fn fused_seam_stereo() -> Vec<f32> {
    samples(&assets::beat_input_fused_seam_stereo())
}
