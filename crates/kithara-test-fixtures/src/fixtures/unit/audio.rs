use std::sync::OnceLock;

use kithara_platform::sync::Arc;
use kithara_test_macros as kithara;

use crate::{assets, fixtures::samples};

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
