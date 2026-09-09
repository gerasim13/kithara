use kithara_platform::{sync::RwLock, time::Duration, traits::FromWithParams};
use kithara_stream::{AudioCodec, ContainerFormat};
use url::Url;

use crate::{
    ids::VariantIndex,
    playlist::parse::{MediaPlaylist, VariantStream},
};

/// Per-segment parsed data (from media playlist).
#[derive(Debug, Clone)]
pub struct SegmentState {
    /// Duration of the segment.
    pub duration: Duration,
    /// Byte length from `#EXT-X-BYTERANGE`, when present.
    pub byte_range_len: Option<u64>,
    /// Absolute URL of the segment.
    pub url: Url,
}

/// Per-variant parsed data.
#[derive(Debug)]
pub struct VariantState {
    /// Audio codec (parsed from CODECS attribute).
    pub codec: Option<AudioCodec>,
    /// Container format (detected from segment URIs).
    pub container: Option<ContainerFormat>,
    /// Absolute URL of the init segment (fMP4 only).
    pub init_url: Option<Url>,
    /// Parsed segments from the media playlist.
    pub segments: Vec<SegmentState>,
}

/// Holds parsed playlist data.
pub struct PlaylistState {
    bandwidths_bps: Vec<Option<u64>>,
    variants: Vec<RwLock<VariantState>>,
}

impl PlaylistState {
    /// Create a new playlist state from parsed variants.
    #[must_use]
    pub fn new(variants: Vec<VariantState>) -> Self {
        let bandwidths_bps = vec![None; variants.len()];
        Self::with_bandwidths(variants, bandwidths_bps)
    }

    fn with_bandwidths(variants: Vec<VariantState>, bandwidths_bps: Vec<Option<u64>>) -> Self {
        Self {
            bandwidths_bps,
            variants: variants.into_iter().map(RwLock::new).collect(),
        }
    }
}

/// Build the live [`PlaylistState`] from parsed master variants + media
/// playlists.
impl FromWithParams<&[VariantStream], &[MediaPlaylist]> for PlaylistState {
    /// Each entry in `media_playlists` corresponds to the variant at the same
    /// index in `variants`. Segment URIs are resolved against the media
    /// playlist URL; failures fall back to the media URL itself.
    fn build(variants: &[VariantStream], media_playlists: &[MediaPlaylist]) -> Self {
        let mut bandwidths_bps: Vec<Option<u64>> = Vec::with_capacity(variants.len());
        let variant_states: Vec<VariantState> = variants
            .iter()
            .zip(media_playlists.iter())
            .map(|(variant, playlist)| {
                bandwidths_bps.push(variant.bandwidth);
                let media_url = &playlist.url;
                let codec = variant.codec.as_ref().and_then(|ci| ci.audio_codec);
                let container = variant
                    .codec
                    .as_ref()
                    .and_then(|ci| ci.container)
                    .or(playlist.detected_container);

                let init_url = playlist
                    .init_segment
                    .as_ref()
                    .and_then(|init| media_url.join(&init.uri).ok());

                let segments: Vec<SegmentState> = playlist
                    .segments
                    .iter()
                    .map(|seg| {
                        let url = media_url
                            .join(&seg.uri)
                            .unwrap_or_else(|_| media_url.clone());
                        SegmentState {
                            url,
                            duration: seg.duration,
                            byte_range_len: seg.byte_range_len,
                        }
                    })
                    .collect();

                VariantState {
                    codec,
                    container,
                    init_url,
                    segments,
                }
            })
            .collect();

        Self::with_bandwidths(variant_states, bandwidths_bps)
    }
}

impl PlaylistState {
    pub(crate) fn init_url(&self, variant: VariantIndex) -> Option<Url> {
        let lock = self.variants.get(variant)?;
        let state = lock.read();
        state.init_url.clone()
    }

    /// Number of segments in a variant's media playlist.
    #[must_use]
    pub fn num_segments(&self, variant: VariantIndex) -> Option<usize> {
        let lock = self.variants.get(variant)?;
        let state = lock.read();
        Some(state.segments.len())
    }

    pub(crate) fn segment_byte_range_len(
        &self,
        variant: VariantIndex,
        index: usize,
    ) -> Option<u64> {
        let lock = self.variants.get(variant)?;
        let state = lock.read();
        state.segments.get(index)?.byte_range_len
    }

    pub(crate) fn segment_decode_range(
        &self,
        variant: VariantIndex,
        index: usize,
    ) -> Option<(Duration, Duration)> {
        let lock = self.variants.get(variant)?;
        let state = lock.read();
        if index >= state.segments.len() {
            return None;
        }
        let result = state
            .segments
            .iter()
            .scan(Duration::ZERO, |elapsed, segment| {
                let start = *elapsed;
                let end = start.saturating_add(segment.duration);
                *elapsed = end;
                Some((start, end))
            })
            .nth(index);
        drop(state);
        result
    }

    pub(crate) fn segment_url(&self, variant: VariantIndex, index: usize) -> Option<Url> {
        let lock = self.variants.get(variant)?;
        let state = lock.read();
        state.segments.get(index).map(|s| s.url.clone())
    }

    /// Total media duration across parsed variants.
    ///
    /// Variants are expected to describe the same timeline. We keep the
    /// maximum duration to tolerate incomplete alternate renditions.
    #[must_use]
    pub fn track_duration(&self) -> Option<Duration> {
        self.variants
            .iter()
            .map(|lock| {
                let state = lock.read();
                state.segments.iter().fold(Duration::ZERO, |acc, segment| {
                    acc.saturating_add(segment.duration)
                })
            })
            .max()
    }

    /// Advertised variant bandwidth from the master playlist, in bits per
    /// second.
    pub(crate) fn variant_bandwidth_bps(&self, variant: VariantIndex) -> Option<u64> {
        self.bandwidths_bps.get(variant).copied().flatten()
    }

    /// Audio codec for a variant.
    pub(crate) fn variant_codec(&self, variant: VariantIndex) -> Option<AudioCodec> {
        let lock = self.variants.get(variant)?;
        let state = lock.read();
        state.codec
    }

    /// Container format for a variant.
    pub(crate) fn variant_container(&self, variant: VariantIndex) -> Option<ContainerFormat> {
        let lock = self.variants.get(variant)?;
        let state = lock.read();
        state.container
    }
}

#[cfg(test)]
impl PlaylistState {
    /// Number of variants in the master playlist.
    fn num_variants(&self) -> usize {
        self.variants.len()
    }
}

#[cfg(test)]
mod tests {
    use kithara_platform::time::Duration;
    use kithara_test_utils::kithara;
    use url::Url;

    use super::*;
    use crate::playlist::parse::{
        CodecInfo, InitSegment, MediaPlaylist, MediaSegment, VariantId, VariantStream,
    };

    fn base_url() -> Url {
        Url::parse("https://cdn.example.com/audio/").expect("valid base URL")
    }

    fn test_url(raw: &str) -> Url {
        Url::parse(raw).expect("valid test URL")
    }

    fn make_segment(index: usize) -> SegmentState {
        SegmentState {
            url: base_url()
                .join(&format!("segment-{index}.m4s"))
                .expect("valid segment URL"),
            duration: Duration::from_secs(4),
            byte_range_len: None,
        }
    }

    fn make_variant_with_byte_ranges(
        _id: usize,
        byte_ranges: impl IntoIterator<Item = Option<u64>>,
    ) -> VariantState {
        let ranges: Vec<Option<u64>> = byte_ranges.into_iter().collect();
        let segments: Vec<SegmentState> = ranges
            .iter()
            .copied()
            .enumerate()
            .map(|(idx, byte_range_len)| {
                let mut segment = make_segment(idx);
                segment.byte_range_len = byte_range_len;
                segment
            })
            .collect();
        VariantState {
            segments,
            codec: Some(AudioCodec::AacLc),
            container: Some(ContainerFormat::Fmp4),
            init_url: Some(base_url().join("init.mp4").expect("valid init URL")),
        }
    }

    fn make_variant(id: usize, num_segments: usize) -> VariantState {
        make_variant_with_byte_ranges(id, (0..num_segments).map(|_| None))
    }

    #[kithara::test]
    fn test_playlist_state_basic_access() {
        let state = PlaylistState::new(vec![make_variant(0, 5), make_variant(1, 3)]);

        assert_eq!(state.num_variants(), 2);
        assert_eq!(state.num_segments(0), Some(5));
        assert_eq!(state.num_segments(1), Some(3));

        assert_eq!(state.num_segments(2), None);
        assert_eq!(state.num_segments(99), None);
    }

    #[kithara::test]
    fn test_playlist_state_variant_info() {
        let mut v = make_variant(0, 2);
        v.codec = Some(AudioCodec::Flac);
        v.container = Some(ContainerFormat::Fmp4);
        let state = PlaylistState::new(vec![v]);

        assert_eq!(state.variant_codec(0), Some(AudioCodec::Flac));
        assert_eq!(state.variant_container(0), Some(ContainerFormat::Fmp4));

        let url = state.segment_url(0, 0).unwrap();
        assert!(url.as_str().contains("segment-0.m4s"));
        let url1 = state.segment_url(0, 1).unwrap();
        assert!(url1.as_str().contains("segment-1.m4s"));
        assert_eq!(state.segment_url(0, 2), None);

        let init = state.init_url(0).unwrap();
        assert!(init.as_str().contains("init.mp4"));

        assert_eq!(state.variant_codec(5), None);
        assert_eq!(state.variant_container(5), None);
        assert_eq!(state.segment_url(5, 0), None);
        assert_eq!(state.init_url(5), None);
    }

    #[kithara::test]
    fn test_segment_byte_range_len_absent() {
        let state = PlaylistState::new(vec![make_variant(0, 3)]);

        assert_eq!(state.segment_byte_range_len(0, 0), None);
        assert_eq!(state.segment_byte_range_len(0, 2), None);
        assert_eq!(state.segment_byte_range_len(0, 3), None);
    }

    #[kithara::test]
    fn test_segment_byte_ranges_surface_as_segment_sizes() {
        let state = PlaylistState::new(vec![make_variant_with_byte_ranges(
            0,
            [Some(200), Some(300), Some(400), Some(500)],
        )]);

        assert_eq!(state.segment_byte_range_len(0, 0), Some(200));
        assert_eq!(state.segment_byte_range_len(0, 3), Some(500));
    }

    #[kithara::test]
    fn test_mixed_byte_ranges_do_not_mint_complete_segment_sizes() {
        let state = PlaylistState::new(vec![make_variant_with_byte_ranges(0, [Some(200), None])]);

        assert_eq!(state.segment_byte_range_len(0, 0), Some(200));
        assert_eq!(state.segment_byte_range_len(0, 1), None);
    }

    #[kithara::test]
    fn test_track_duration_uses_longest_variant() {
        let state = PlaylistState::new(vec![make_variant(0, 4), make_variant(1, 3)]);
        assert_eq!(state.track_duration(), Some(Duration::from_secs(16)));
    }

    #[kithara::test(wasm)]
    #[case::first(0, Some((Duration::from_secs(0), Duration::from_secs(4))))]
    #[case::second(1, Some((Duration::from_secs(4), Duration::from_secs(8))))]
    #[case::third(2, Some((Duration::from_secs(8), Duration::from_secs(12))))]
    #[case::last(3, Some((Duration::from_secs(12), Duration::from_secs(16))))]
    #[case::out_of_range(4, None)]
    fn test_segment_decode_range(
        #[case] index: usize,
        #[case] expected: Option<(Duration, Duration)>,
    ) {
        let state = PlaylistState::new(vec![make_variant(0, 4)]);
        assert_eq!(state.segment_decode_range(0, index), expected);
    }

    #[kithara::test]
    fn test_from_parsed_basic() {
        let variants = [VariantStream {
            id: VariantId(0),
            uri: "v0.m3u8".to_string(),
            bandwidth: Some(128_000),
            name: None,
            codec: Some(CodecInfo {
                codecs: Some("mp4a.40.2".to_string()),
                audio_codec: Some(AudioCodec::AacLc),
                container: Some(ContainerFormat::Fmp4),
            }),
        }];

        let playlist = MediaPlaylist {
            url: test_url("https://cdn.example.com/audio/v0.m3u8"),
            segments: vec![
                MediaSegment {
                    sequence: 0,
                    uri: "segment-0.m4s".to_string(),
                    duration: Duration::from_secs(4),
                    key: None,
                    byte_range_len: None,
                },
                MediaSegment {
                    sequence: 1,
                    uri: "segment-1.m4s".to_string(),
                    duration: Duration::from_secs(4),
                    key: None,
                    byte_range_len: None,
                },
            ],
            init_segment: Some(InitSegment {
                uri: "init.mp4".to_string(),
                key: None,
            }),
            detected_container: Some(ContainerFormat::Fmp4),
        };

        let media_playlists = [playlist];
        let state = PlaylistState::build(&variants[..], &media_playlists[..]);

        assert_eq!(state.num_variants(), 1);

        assert_eq!(state.num_segments(0), Some(2));

        assert_eq!(state.variant_codec(0), Some(AudioCodec::AacLc));
        assert_eq!(state.variant_container(0), Some(ContainerFormat::Fmp4));

        let seg0_url = state.segment_url(0, 0).unwrap();
        assert!(
            seg0_url.as_str().contains("segment-0.m4s"),
            "segment URL should contain segment-0.m4s, got: {seg0_url}"
        );

        let seg1_url = state.segment_url(0, 1).unwrap();
        assert!(
            seg1_url.as_str().contains("segment-1.m4s"),
            "segment URL should contain segment-1.m4s, got: {seg1_url}"
        );

        let init = state.init_url(0).unwrap();
        assert!(
            init.as_str().contains("init.mp4"),
            "init URL should contain init.mp4, got: {init}"
        );

        assert_eq!(state.segment_byte_range_len(0, 0), None);
    }
}
