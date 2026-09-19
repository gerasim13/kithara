use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared build-time PCM input for resampled markers.
#[kithara::fixture]
#[must_use]
pub fn resampled_markers() -> Vec<f32> {
    samples(&assets::unit_pcm_resampled_markers())
}

/// Prepared build-time WAV input.
#[kithara::fixture]
#[must_use]
pub fn resampled_wav_four() -> &'static [u8] {
    assets::resampled_wav_four().bytes()
}

/// Prepared build-time WAV input.
#[kithara::fixture]
#[must_use]
pub fn resampled_wav_eight() -> &'static [u8] {
    assets::resampled_wav_eight().bytes()
}

/// Prepared build-time WAV input.
#[kithara::fixture]
#[must_use]
pub fn resampled_wav_seek() -> &'static [u8] {
    assets::resampled_wav_seek().bytes()
}

/// Prepared build-time WAV input.
#[kithara::fixture]
#[must_use]
pub fn poisoned_float_wav() -> &'static [u8] {
    assets::poisoned_float_wav_data().bytes()
}

/// Prepared build-time FLAC input with unknown container duration.
#[cfg(feature = "encoded")]
#[kithara::fixture]
#[must_use]
pub fn flac_saw() -> &'static [u8] {
    assets::flac_unknown_length_saw_6s().bytes()
}
