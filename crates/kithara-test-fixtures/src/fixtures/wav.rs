#![cfg(feature = "wav")]

use kithara_test_macros as kithara;

use crate::assets;

/// Prepared full-scale stereo 440 Hz WAV input.
#[cfg(not(target_arch = "wasm32"))]
#[kithara::fixture]
#[must_use]
pub fn short_decoder_wav() -> &'static [u8] {
    assets::sine_wav_a440_10_frames().bytes()
}

/// Prepared full-scale stereo 440 Hz WAV input.
#[cfg(not(target_arch = "wasm32"))]
#[kithara::fixture]
#[must_use]
pub fn decoder_wav() -> &'static [u8] {
    assets::sine_wav_a440_100_frames().bytes()
}

/// Prepared full-scale stereo 440 Hz WAV input.
#[cfg(not(target_arch = "wasm32"))]
#[kithara::fixture]
#[must_use]
pub fn seek_decoder_wav() -> &'static [u8] {
    assets::sine_wav_a440_10000_frames().bytes()
}

/// Prepared full-scale stereo 440 Hz WAV input.
#[kithara::fixture]
#[must_use]
pub fn stress_wav() -> &'static [u8] {
    assets::timeline_wav_default().bytes()
}
