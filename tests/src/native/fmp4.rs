use std::borrow::Cow;

use kithara::platform::sync::Arc;
use kithara_encode::EncodedTrack;
use kithara_test_fixtures::fmp4::{Fmp4MuxError, GaplessEncoding, mux_audio_track};

use crate::rfc6381::Rfc6381Ext;

/// One muxed fMP4 variant plus the `CODECS` attribute its playlist entry needs.
///
/// `kithara-test-fixtures` builds the boxes; the RFC 6381 string is a manifest
/// concern and stays here with the harness that writes playlists. The segments
/// are shared because every request for the same variant hands out the same
/// bytes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PackagedVariantData {
    pub(crate) init_segment: Arc<Vec<u8>>,
    pub(crate) rfc6381_codec: Cow<'static, str>,
    pub(crate) media_segments: Vec<Arc<Vec<u8>>>,
    pub(crate) segment_durations_secs: Vec<f64>,
}

pub(crate) fn mux_packaged_variant(
    track: &EncodedTrack,
    gapless_encoding: GaplessEncoding,
) -> Result<PackagedVariantData, Fmp4MuxError> {
    let package = mux_audio_track(track, gapless_encoding)?;
    let codec = track
        .media_info
        .codec
        .ok_or(Fmp4MuxError::InvalidMediaInfo)?;
    let rfc6381_codec = track
        .media_info
        .rfc6381_codec()
        .ok_or(Fmp4MuxError::UnsupportedCodec(codec))?;

    Ok(PackagedVariantData {
        init_segment: Arc::new(package.init_segment),
        rfc6381_codec,
        media_segments: package.media_segments.into_iter().map(Arc::new).collect(),
        segment_durations_secs: package.segment_durations_secs,
    })
}
