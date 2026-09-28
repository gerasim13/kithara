use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared build-time PCM input for accelerate ramp.
#[kithara::fixture]
#[must_use]
pub fn accelerate_ramp() -> Vec<f32> {
    samples(&assets::unit_pcm_accelerate_ramp())
}

/// Prepared build-time PCM input for accelerate wave.
#[kithara::fixture]
#[must_use]
pub fn accelerate_wave() -> Vec<f32> {
    samples(&assets::unit_pcm_accelerate_wave())
}
