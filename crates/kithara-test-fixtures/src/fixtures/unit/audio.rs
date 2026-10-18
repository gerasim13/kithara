use std::sync::OnceLock;

use kithara_platform::sync::Arc;
use kithara_test_macros as kithara;

use crate::{asset::Asset, assets, fixtures::samples};

/// Prepared build-time PCM input for blend identity.
#[kithara::fixture]
#[must_use]
pub fn blend_identity() -> Vec<f32> {
    samples(&assets::unit_pcm_blend_identity())
}

/// Prepared build-time PCM input for blend multichannel.
#[kithara::fixture]
#[must_use]
pub fn blend_multichannel() -> Vec<f32> {
    samples(&assets::unit_pcm_blend_multichannel())
}

/// Prepared build-time PCM input for blend outgoing.
#[kithara::fixture]
#[must_use]
pub fn blend_outgoing() -> Vec<f32> {
    samples(&assets::unit_pcm_blend_outgoing())
}

/// Prepared build-time PCM input for blend incoming.
#[kithara::fixture]
#[must_use]
pub fn blend_incoming() -> Vec<f32> {
    samples(&assets::unit_pcm_blend_incoming())
}

/// Prepared build-time PCM input for blend outgoing constant.
#[kithara::fixture]
#[must_use]
pub fn blend_outgoing_constant() -> Vec<f32> {
    samples(&assets::unit_pcm_blend_outgoing_constant())
}

/// Prepared build-time PCM input for blend join frame.
#[kithara::fixture]
#[must_use]
pub fn blend_join_frame() -> Vec<f32> {
    samples(&assets::unit_pcm_blend_join_frame())
}

/// Prepared build-time PCM input for blend signed frame.
#[kithara::fixture]
#[must_use]
pub fn blend_signed_frame() -> Vec<f32> {
    samples(&assets::unit_pcm_blend_signed_frame())
}

/// Prepared build-time PCM input for cursor half.
#[kithara::fixture]
#[must_use]
pub fn cursor_half() -> Vec<f32> {
    samples(&assets::unit_pcm_cursor_half())
}

/// Prepared build-time PCM input for decode quarter.
#[kithara::fixture]
#[must_use]
pub fn decode_quarter() -> Vec<f32> {
    samples(&assets::unit_pcm_decode_quarter())
}

/// Prepared build-time PCM input for decode negative quarter.
#[kithara::fixture]
#[must_use]
pub fn decode_negative_quarter() -> Vec<f32> {
    samples(&assets::unit_pcm_decode_negative_quarter())
}

/// Prepared stereo tones at the two rates exercised by route-change tests.
pub type RoutePcm = [Arc<[f32]>; 2];

#[kithara::fixture]
pub fn route_pcm() -> RoutePcm {
    static PCM: OnceLock<RoutePcm> = OnceLock::new();
    PCM.get_or_init(|| {
        [
            assets::unit_pcm_route_44100(),
            assets::unit_pcm_route_48000(),
        ]
        .map(|asset| samples(&asset).into())
    })
    .clone()
}

/// The six-second mono broadcast tone repeated into stereo float32 WAV.
#[must_use]
pub fn broadcast_tone_wav() -> Asset {
    assets::broadcast_tone_wav_default()
}

/// Twenty seconds of stereo 440 Hz sine, at full or half fixture amplitude.
#[must_use]
pub fn metronome_sine_wav(half: bool) -> Asset {
    if half {
        assets::metronome_sine_wav_half()
    } else {
        assets::metronome_sine_wav_full()
    }
}

/// A complete 24-second plain or decorated control at 44.1 or 48 kHz.
///
/// # Panics
/// Panics if the rate is outside the control case matrix.
#[must_use]
pub fn cochlea_control_wav(sample_rate: u32, decorated: bool) -> Asset {
    match (sample_rate, decorated) {
        (44_100, false) => assets::cochlea_control_wav_cd_plain(),
        (44_100, true) => assets::cochlea_control_wav_cd_decorated(),
        (48_000, false) => assets::cochlea_control_wav_render_plain(),
        (48_000, true) => assets::cochlea_control_wav_render_decorated(),
        _ => panic!("unsupported Cochlea control rate {sample_rate}"),
    }
}
