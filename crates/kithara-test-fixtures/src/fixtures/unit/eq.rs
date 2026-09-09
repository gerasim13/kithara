use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared build-time PCM input for eq sine 40.
#[kithara::fixture]
#[must_use]
pub fn eq_sine_40() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_sine_40())
}

/// Prepared build-time PCM input for eq sine 1000.
#[kithara::fixture]
#[must_use]
pub fn eq_sine_1000() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_sine_1000())
}

/// Prepared build-time PCM input for eq sine 10000.
#[kithara::fixture]
#[must_use]
pub fn eq_sine_10000() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_sine_10000())
}

/// Prepared build-time PCM input for eq sine 15000.
#[kithara::fixture]
#[must_use]
pub fn eq_sine_15000() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_sine_15000())
}

/// Prepared build-time PCM input for eq silence.
#[kithara::fixture]
#[must_use]
pub fn eq_silence() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_silence())
}

/// Prepared build-time PCM input for eq half.
#[kithara::fixture]
#[must_use]
pub fn eq_half() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_half())
}

/// Prepared build-time PCM input for eq finite.
#[kithara::fixture]
#[must_use]
pub fn eq_finite() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_finite())
}

/// Prepared build-time PCM input for eq oscillation.
#[kithara::fixture]
#[must_use]
pub fn eq_oscillation() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_oscillation())
}

/// Prepared build-time PCM input for eq bypass.
#[kithara::fixture]
#[must_use]
pub fn eq_bypass() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_bypass())
}

/// Prepared build-time PCM input for eq transition.
#[kithara::fixture]
#[must_use]
pub fn eq_transition() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_transition())
}

/// Prepared build-time PCM input for eq impulse.
#[kithara::fixture]
#[must_use]
pub fn eq_impulse() -> Vec<f32> {
    samples(&assets::unit_pcm_eq_impulse())
}
