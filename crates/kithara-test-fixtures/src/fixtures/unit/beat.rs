use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared build-time PCM input for clicks 120 4s.
#[kithara::fixture]
#[must_use]
pub fn clicks_120_4s() -> Vec<f32> {
    samples(&assets::unit_pcm_clicks_120_4s())
}

/// Prepared build-time PCM input for clicks 120 20s.
#[kithara::fixture]
#[must_use]
pub fn clicks_120_20s() -> Vec<f32> {
    samples(&assets::unit_pcm_clicks_120_20s())
}

/// Prepared build-time PCM input for clicks 150 12s.
#[kithara::fixture]
#[must_use]
pub fn clicks_150_12s() -> Vec<f32> {
    samples(&assets::unit_pcm_clicks_150_12s())
}

/// Prepared build-time PCM input for clicks 75 20s.
#[kithara::fixture]
#[must_use]
pub fn clicks_75_20s() -> Vec<f32> {
    samples(&assets::unit_pcm_clicks_75_20s())
}

/// Prepared build-time PCM input for clicks 90 20s.
#[kithara::fixture]
#[must_use]
pub fn clicks_90_20s() -> Vec<f32> {
    samples(&assets::unit_pcm_clicks_90_20s())
}

/// Prepared build-time PCM input for clicks 150 20s.
#[kithara::fixture]
#[must_use]
pub fn clicks_150_20s() -> Vec<f32> {
    samples(&assets::unit_pcm_clicks_150_20s())
}

/// Prepared build-time PCM input for clicks change 40s.
#[kithara::fixture]
#[must_use]
pub fn clicks_change_40s() -> Vec<f32> {
    samples(&assets::unit_pcm_clicks_change_40s())
}

/// Prepared build-time PCM input for clicks change 24s.
#[kithara::fixture]
#[must_use]
pub fn clicks_change_24s() -> Vec<f32> {
    samples(&assets::unit_pcm_clicks_change_24s())
}

/// Prepared build-time PCM input for click silence 4s.
#[kithara::fixture]
#[must_use]
pub fn click_silence_4s() -> Vec<f32> {
    samples(&assets::unit_pcm_click_silence_4s())
}

/// Prepared build-time PCM input for click silence 20s.
#[kithara::fixture]
#[must_use]
pub fn click_silence_20s() -> Vec<f32> {
    samples(&assets::unit_pcm_click_silence_20s())
}

/// Prepared build-time PCM input for click silence half.
#[kithara::fixture]
#[must_use]
pub fn click_silence_half() -> Vec<f32> {
    samples(&assets::unit_pcm_click_silence_half())
}
