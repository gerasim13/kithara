use kithara_test_macros as kithara;

use crate::{asset::Asset, assets};

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

/// Prepared small ascending PCM input, generated and embedded at build time.
#[kithara::fixture]
#[must_use]
pub fn pcm_ramp() -> Vec<f32> {
    samples(&assets::pcm_ramp())
}

/// Prepared negative ascending-magnitude PCM input.
#[kithara::fixture]
#[must_use]
pub fn negative_pcm_ramp() -> Vec<f32> {
    samples(&assets::pcm_negative_ramp())
}

/// Two stereo frames whose channel identities are distinct.
#[kithara::fixture]
#[must_use]
pub fn stereo_pair() -> Vec<f32> {
    samples(&assets::pcm_stereo_pair())
}

/// Five labeled frames for each channel count from one through nine.
#[kithara::fixture]
#[must_use]
pub fn channel_signals() -> [Vec<f32>; 9] {
    [
        assets::channel_labels_mono(),
        assets::channel_labels_stereo(),
        assets::channel_labels_three(),
        assets::channel_labels_four(),
        assets::channel_labels_five(),
        assets::channel_labels_six(),
        assets::channel_labels_seven(),
        assets::channel_labels_eight(),
        assets::channel_labels_nine(),
    ]
    .map(|asset| samples(&asset))
}

pub(crate) fn samples(asset: &Asset) -> Vec<f32> {
    let bytes = asset.bytes();
    let chunks = bytes.chunks_exact(size_of::<f32>());
    assert!(
        chunks.remainder().is_empty(),
        "fixture contains complete f32 samples"
    );
    chunks
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("one f32 sample")))
        .collect()
}

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

/// Six prepared silent PCM samples.
#[kithara::fixture]
#[must_use]
pub fn silence_pcm() -> Vec<f32> {
    samples(&assets::pcm_silence())
}

/// Prepared PCM for signal provenance checks.
#[kithara::fixture]
#[must_use]
pub fn ascending_pcm() -> Vec<f32> {
    samples(&assets::provenance_ascending())
}

/// Prepared PCM for signal provenance checks.
#[kithara::fixture]
#[must_use]
pub fn ascending_wrap_pcm() -> Vec<f32> {
    samples(&assets::provenance_ascending_wrap())
}

/// Prepared PCM for signal provenance checks.
#[kithara::fixture]
#[must_use]
pub fn descending_pcm() -> Vec<f32> {
    samples(&assets::provenance_descending())
}

/// Prepared PCM for signal provenance checks.
#[kithara::fixture]
#[must_use]
pub fn descending_wrap_pcm() -> Vec<f32> {
    samples(&assets::provenance_descending_wrap())
}

/// Prepared PCM for signal provenance checks.
#[kithara::fixture]
#[must_use]
pub fn provenance_silence() -> Vec<f32> {
    samples(&assets::pcm_provenance_silence())
}

/// Prepared PCM for signal provenance checks.
#[kithara::fixture]
#[must_use]
pub fn phase_endpoints() -> Vec<f32> {
    samples(&assets::pcm_phase_endpoints())
}

#[kithara::fixture]
#[must_use]
pub fn direction_step() -> Vec<f32> {
    samples(&assets::pcm_direction_step())
}

#[kithara::fixture]
#[must_use]
pub fn direction_channel_less() -> Vec<f32> {
    samples(&assets::pcm_direction_channel_less())
}
