#![cfg(feature = "signal")]

use kithara_test_macros as kithara;

use crate::assets;

/// Prepared bytes of the build-time generated 187-second MPEG tone.
#[kithara::fixture]
#[must_use]
pub fn tone_mp3() -> &'static [u8] {
    assets::signal_mp3_track_sine440_187s().bytes()
}

/// Prepared bytes of the build-time generated one-second stereo WAV tone.
#[cfg(not(target_arch = "wasm32"))]
#[kithara::fixture]
#[must_use]
pub fn tone_wav() -> &'static [u8] {
    assets::signal_wav_sine440_1s().bytes()
}
