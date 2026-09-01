use kithara_platform::sync::Arc;
use kithara_signal::AudioSpec;

use crate::{
    error::DecodeError,
    gapless::{GaplessInfo, GaplessTailCompensation},
};

/// Decoder-owned per-track playback contract.
///
/// `#[non_exhaustive]` because callers construct it with
/// `..Default::default()` and the decoder may add further track-level facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecoderTrackInfo {
    /// Gapless trim information applied by the engine pipeline.
    pub gapless: Option<GaplessInfo>,
    /// Fused sample-rate-conversion tail compensation.
    pub gapless_tail: Option<GaplessTailCompensation>,
}

/// Immutable decoder facts consumed when constructing a gapless trimmer.
///
/// This profile references the existing gapless contracts instead of copying
/// their frame counts into another source of truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct GaplessProfile {
    spec: AudioSpec,
    gapless: Option<GaplessInfo>,
    tail_compensation: Option<GaplessTailCompensation>,
    default_priming_frames: u64,
}

impl GaplessProfile {
    #[must_use]
    pub const fn new(
        spec: AudioSpec,
        gapless: Option<GaplessInfo>,
        tail_compensation: Option<GaplessTailCompensation>,
        default_priming_frames: u64,
    ) -> Self {
        Self {
            spec,
            gapless,
            tail_compensation,
            default_priming_frames,
        }
    }

    #[must_use]
    pub const fn default_priming_frames(self) -> u64 {
        self.default_priming_frames
    }

    #[must_use]
    pub const fn gapless(self) -> Option<GaplessInfo> {
        self.gapless
    }

    #[must_use]
    pub const fn spec(self) -> AudioSpec {
        self.spec
    }

    #[must_use]
    pub const fn tail_compensation(self) -> Option<GaplessTailCompensation> {
        self.tail_compensation
    }
}

/// Construction-time decoded-audio specification for a gapless blender.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlenderProfile {
    spec: AudioSpec,
}

impl BlenderProfile {
    #[must_use]
    pub const fn new(spec: AudioSpec) -> Self {
        Self { spec }
    }

    #[must_use]
    pub const fn spec(self) -> AudioSpec {
        self.spec
    }
}

/// Audio track metadata extracted from decoder tags.
///
/// Intentionally without `#[non_exhaustive]`: downstream fixtures and
/// processors construct this stable optional-field value with struct literals.
#[derive(Debug, Clone, Default)]
pub struct TrackMetadata {
    /// Album name.
    pub album: Option<String>,
    /// Artist name.
    pub artist: Option<String>,
    /// Album artwork bytes.
    pub artwork: Option<Arc<Vec<u8>>>,
    /// Track title.
    pub title: Option<String>,
}

pub(crate) fn checked_audio_spec(
    channels: u16,
    sample_rate: u32,
    resource: &'static str,
) -> Result<AudioSpec, DecodeError> {
    let sample_rate = std::num::NonZeroU32::new(sample_rate)
        .ok_or(DecodeError::InvalidSampleRate { resource })?;
    Ok(AudioSpec::new(channels, sample_rate))
}
