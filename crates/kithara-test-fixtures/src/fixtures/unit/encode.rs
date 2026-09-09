use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared build-time PCM input for encode saw.
#[kithara::fixture]
#[must_use]
pub fn encode_saw() -> Vec<f32> {
    samples(&assets::unit_pcm_encode_saw())
}

/// Prepared build-time PCM input for encode session.
#[kithara::fixture]
#[must_use]
pub fn encode_session() -> Vec<f32> {
    samples(&assets::unit_pcm_encode_session())
}

/// Prepared build-time PCM input for record labels.
#[kithara::fixture]
#[must_use]
pub fn record_labels() -> Vec<f32> {
    samples(&assets::unit_pcm_record_labels())
}

/// Prepared build-time PCM input for record signed.
#[kithara::fixture]
#[must_use]
pub fn record_signed() -> Vec<f32> {
    samples(&assets::unit_pcm_record_signed())
}

/// Prepared build-time WAV input.
#[kithara::fixture]
#[must_use]
pub fn encode_saw_i16() -> &'static [u8] {
    assets::encode_saw_i16_data().bytes()
}

/// Prepared build-time WAV input.
#[kithara::fixture]
#[must_use]
pub fn encode_scale_i16() -> &'static [u8] {
    assets::encode_scale_i16_data().bytes()
}

/// Prepared build-time WAV input.
#[kithara::fixture]
#[must_use]
pub fn encode_partial_i16() -> &'static [u8] {
    assets::encode_partial_i16_data().bytes()
}
