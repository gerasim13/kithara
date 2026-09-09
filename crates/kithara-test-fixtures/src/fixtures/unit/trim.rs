use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

/// Prepared build-time PCM input for trim ramp.
#[kithara::fixture]
#[must_use]
pub fn trim_ramp() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_ramp())
}

/// Prepared build-time PCM input for trim silence.
#[kithara::fixture]
#[must_use]
pub fn trim_silence() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence())
}

/// Prepared build-time PCM input for trim codec priming drops leading frames and fades in.
#[kithara::fixture]
#[must_use]
pub fn trim_codec_priming_drops_leading_frames_and_fades_in() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_codec_priming_drops_leading_frames_and_fades_in())
}

/// Prepared build-time PCM input for trim codec priming metadata takes precedence when combined.
#[kithara::fixture]
#[must_use]
pub fn trim_codec_priming_metadata_takes_precedence_when_combined() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_codec_priming_metadata_takes_precedence_when_combined())
}

/// Prepared build-time PCM input for trim silence trim below threshold is trimmed.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_below_threshold_is_trimmed() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_below_threshold_is_trimmed())
}

/// Prepared build-time PCM input for trim silence trim above threshold preserves audio.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_above_threshold_preserves_audio() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_above_threshold_preserves_audio())
}

/// Prepared build-time PCM input for trim silence trim preserves quiet intro below threshold then above.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_preserves_quiet_intro_below_threshold_then_above() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_preserves_quiet_intro_below_threshold_then_above())
}

/// Prepared build-time PCM input for trim silence trim min frames boundary under min.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_min_frames_boundary_under_min() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_min_frames_boundary_under_min())
}

/// Prepared build-time PCM input for trim silence trim min frames boundary at min.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_min_frames_boundary_at_min() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_min_frames_boundary_at_min())
}

/// Prepared build-time PCM input for trim silence trim scan window exhausted preserves audio.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_scan_window_exhausted_preserves_audio() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_scan_window_exhausted_preserves_audio())
}

/// Prepared build-time PCM input for trim silence trim no op with immediate content.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_no_op_with_immediate_content() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_no_op_with_immediate_content())
}

/// Prepared build-time PCM input for trim silence trim trailing disabled by default.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_trailing_disabled_by_default() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_trailing_disabled_by_default())
}

/// Prepared build-time PCM input for trim silence trim trailing enabled.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_trailing_enabled() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_trailing_enabled())
}

/// Prepared build-time PCM input for trim trailing sine.
#[kithara::fixture]
#[must_use]
pub fn trim_trailing_sine() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_trailing_sine())
}

/// Prepared build-time PCM input for trim seek.
#[kithara::fixture]
#[must_use]
pub fn trim_seek() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_seek())
}

/// Prepared build-time PCM input for trim stereo silence.
#[kithara::fixture]
#[must_use]
pub fn trim_stereo_silence() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_stereo_silence())
}

/// Prepared build-time PCM input for trim stereo quiet.
#[kithara::fixture]
#[must_use]
pub fn trim_stereo_quiet() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_stereo_quiet())
}

/// Prepared build-time PCM input for trim silence trim does not introduce click at boundary.
#[kithara::fixture]
#[must_use]
pub fn trim_silence_trim_does_not_introduce_click_at_boundary() -> Vec<f32> {
    samples(&assets::unit_pcm_trim_silence_trim_does_not_introduce_click_at_boundary())
}
