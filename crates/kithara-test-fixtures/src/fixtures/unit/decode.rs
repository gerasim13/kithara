#[cfg(not(target_arch = "wasm32"))]
use std::fs;

use kithara_test_macros as kithara;

#[cfg(not(target_arch = "wasm32"))]
use crate::hls;
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

/// Prepared init or media bytes from the build-time HLS bundle.
#[cfg(not(target_arch = "wasm32"))]
#[kithara::fixture]
#[must_use]
/// # Panics
/// Panics if the build-time HLS manifest omits the required resource.
pub fn aac_init() -> Vec<u8> {
    let resource = hls::long_plain()
        .get("/hls/init-slq-a1.mp4")
        .expect("prepared HLS resource");
    fs::read(resource.path()).expect("read prepared HLS bytes")
}

/// Prepared init or media bytes from the build-time HLS bundle.
#[cfg(not(target_arch = "wasm32"))]
#[kithara::fixture]
#[must_use]
/// # Panics
/// Panics if the build-time HLS manifest omits the required resource.
pub fn aac_segment() -> Vec<u8> {
    let resource = hls::long_plain()
        .get("/hls/segment-1-slq-a1.m4s")
        .expect("prepared HLS resource");
    fs::read(resource.path()).expect("read prepared HLS bytes")
}

/// Prepared init or media bytes from the build-time HLS bundle.
#[cfg(not(target_arch = "wasm32"))]
#[kithara::fixture]
#[must_use]
/// # Panics
/// Panics if the build-time HLS manifest omits the required resource.
pub fn flac_init() -> Vec<u8> {
    let resource = hls::long_plain()
        .get("/hls/init-slossless-a1.mp4")
        .expect("prepared HLS resource");
    fs::read(resource.path()).expect("read prepared HLS bytes")
}

/// Prepared build-time FLAC input with unknown container duration.
#[kithara::fixture]
#[must_use]
pub fn flac_saw() -> &'static [u8] {
    assets::flac_unknown_length_saw_6s().bytes()
}
