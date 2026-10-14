use kithara_integration_tests::kithara;
use kithara_test_fixtures::assets::{marked_sine_wav_a440_6s, sine_wav_a440_6s};

use crate::consts::{
    CHANNELS, RATE_MARKER_FRAMES, RATE_MARKER_PEAK, RATE_MARKER_START_FRAMES, SAMPLE_BYTES,
    SAMPLE_RATE, SOURCE_SECONDS, WAV_HEADER_BYTES,
};

pub(super) fn source_frames() -> usize {
    usize::try_from(SAMPLE_RATE).expect("sample rate fits usize") * SOURCE_SECONDS
}

/// The plain tone this test measures, with the fixture bound to its budget.
///
/// # Panics
///
/// Panics when the fixture no longer matches the test's frame budget.
#[kithara::fixture]
#[must_use]
pub fn source_pcm() -> &'static [u8] {
    let bytes = sine_wav_a440_6s().bytes();
    assert_eq!(
        bytes.len(),
        WAV_HEADER_BYTES + source_frames() * usize::from(CHANNELS) * SAMPLE_BYTES,
        "fixture sine_wav_a440_6s no longer matches this test's frame budget",
    );
    bytes
}

/// The same tone carrying the source-time markers `marker_timing` looks for.
#[kithara::fixture]
pub fn marked_source_pcm(source_pcm: &'static [u8]) -> &'static [u8] {
    let bytes = marked_sine_wav_a440_6s().bytes();
    assert_eq!(bytes.len(), source_pcm.len());
    for start in RATE_MARKER_START_FRAMES {
        assert!(
            peak_amplitude(bytes, start, RATE_MARKER_FRAMES) <= RATE_MARKER_PEAK,
            "fixture marked_sine_wav_a440_6s is loud across the marker at frame {start}",
        );
        assert!(
            peak_amplitude(bytes, start - RATE_MARKER_FRAMES, RATE_MARKER_FRAMES)
                > RATE_MARKER_PEAK,
            "fixture marked_sine_wav_a440_6s has no amplitude step into the marker at frame {start}",
        );
    }
    bytes
}

/// Loudest absolute sample across `frames` of a 16-bit interleaved WAV.
fn peak_amplitude(wav: &[u8], start_frame: usize, frames: usize) -> i16 {
    let channels = usize::from(CHANNELS);
    wav[WAV_HEADER_BYTES..]
        .chunks_exact(SAMPLE_BYTES)
        .skip(start_frame * channels)
        .take(frames * channels)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).saturating_abs())
        .max()
        .expect("the window is inside the fixture")
}
