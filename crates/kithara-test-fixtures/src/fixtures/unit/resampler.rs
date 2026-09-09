use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared build-time PCM input for glide unity.
#[kithara::fixture]
#[must_use]
pub fn glide_unity() -> Vec<f32> {
    samples(&assets::unit_pcm_glide_unity())
}

/// Prepared build-time PCM input for glide quadratic.
#[kithara::fixture]
#[must_use]
pub fn glide_quadratic() -> Vec<f32> {
    samples(&assets::unit_pcm_glide_quadratic())
}

/// Prepared build-time PCM input for glide transition.
#[kithara::fixture]
#[must_use]
pub fn glide_transition() -> Vec<f32> {
    samples(&assets::unit_pcm_glide_transition())
}

/// Prepared build-time PCM input for glide alias.
#[kithara::fixture]
#[must_use]
pub fn glide_alias() -> Vec<f32> {
    samples(&assets::unit_pcm_glide_alias())
}

/// Prepared build-time PCM input for rubato stereo.
#[kithara::fixture]
#[must_use]
pub fn rubato_stereo() -> Vec<f32> {
    samples(&assets::unit_pcm_rubato_stereo())
}

/// Prepared build-time PCM input for rubato nine.
#[kithara::fixture]
#[must_use]
pub fn rubato_nine() -> Vec<f32> {
    samples(&assets::unit_pcm_rubato_nine())
}

/// Prepared build-time PCM input for apple planar 44100.
#[kithara::fixture]
#[must_use]
pub fn apple_planar_44100() -> Vec<f32> {
    samples(&assets::unit_pcm_apple_planar_44100())
}

/// Prepared build-time PCM input for apple planar 48000.
#[kithara::fixture]
#[must_use]
pub fn apple_planar_48000() -> Vec<f32> {
    samples(&assets::unit_pcm_apple_planar_48000())
}
