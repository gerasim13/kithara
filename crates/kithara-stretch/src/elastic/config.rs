use bon::bon;
use kithara_bufpool::PcmPool;
use num_traits::ToPrimitive;

use super::{ElasticError, ElasticRateEnvelope};

/// Numeric continuity policy for exact-span planning.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct ElasticSpanConfig {
    /// Source-frame tolerance used when comparing adjacent continuous spans.
    #[field(get, copy)]
    continuity_tolerance: f64,
    /// Maximum source-frame correction applied over one render block.
    #[field(get, copy)]
    max_correction_per_block: f64,
    /// Maximum source-frame phase error accepted at a block boundary.
    #[field(get, copy)]
    max_phase_error: f64,
}

impl TryFrom<(f64, f64, f64)> for ElasticSpanConfig {
    type Error = ElasticError;

    fn try_from(
        (continuity_tolerance, max_phase_error, max_correction_per_block): (f64, f64, f64),
    ) -> Result<Self, Self::Error> {
        if let Some((field, value)) = [
            ("continuity_tolerance", continuity_tolerance),
            ("max_phase_error", max_phase_error),
            ("max_correction_per_block", max_correction_per_block),
        ]
        .into_iter()
        .find(|(_, value)| !value.is_finite() || *value <= 0.0)
        {
            return Err(ElasticError::InvalidSpanConfig { field, value });
        }
        Ok(Self {
            continuity_tolerance,
            max_correction_per_block,
            max_phase_error,
        })
    }
}

/// Engine preparation resources and fixed frame limits.
#[derive(Clone, Debug, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct ElasticConfig {
    /// Shared PCM pool used by engines that need planar scratch.
    pool: PcmPool,
    #[field(get(copy), vis = "pub(crate)")]
    shape: ElasticShape,
}

#[bon]
impl ElasticConfig {
    /// Builds a validated preparation config with its shared PCM pool.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when a scalar is zero or cannot be represented
    /// by the native engines.
    #[builder(
        builder_type(vis = "pub"),
        start_fn(name = builder, vis = "pub"),
        finish_fn(vis = "pub")
    )]
    fn new(
        pool: PcmPool,
        sample_rate: u32,
        channels: usize,
        max_source_frames: usize,
        max_output_frames: usize,
    ) -> Result<Self, ElasticError> {
        if sample_rate == 0 {
            return Err(ElasticError::InvalidSampleRate);
        }
        let channels = frame_count(
            channels,
            ElasticError::InvalidChannelCount,
            ElasticError::ChannelCountOutOfRange,
        )?;
        let max_source_frames = frame_count(
            max_source_frames,
            ElasticError::InvalidSourceFrameLimit,
            ElasticError::SourceFrameLimitOutOfRange,
        )?;
        let max_output_frames = frame_count(
            max_output_frames,
            ElasticError::InvalidOutputFrameLimit,
            ElasticError::OutputFrameLimitOutOfRange,
        )?;
        let shape = ElasticShape {
            channels,
            max_output_frames,
            max_source_frames,
            sample_rate,
        };
        shape.rate_envelope()?;
        Ok(Self { pool, shape })
    }

    delegate::delegate! {
        to self.shape {
            /// Prepared interleaved channel count.
            #[must_use]
            pub const fn channels(&self) -> usize;
            /// Largest accepted output block in frames.
            #[must_use]
            pub const fn max_output_frames(&self) -> usize;
            /// Largest accepted source block in frames.
            #[must_use]
            pub const fn max_source_frames(&self) -> usize;
            /// Prepared source sample rate in Hz.
            #[must_use]
            pub const fn sample_rate(&self) -> u32;
        }
    }

    /// Shared PCM pool used by engines that need planar scratch.
    #[must_use]
    pub const fn pool(&self) -> &PcmPool {
        &self.pool
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ElasticShape {
    channels: usize,
    max_output_frames: usize,
    max_source_frames: usize,
    sample_rate: u32,
}

impl ElasticShape {
    pub(crate) fn rate_envelope(self) -> Result<ElasticRateEnvelope, ElasticError> {
        let max_output_frames =
            self.max_output_frames
                .to_f64()
                .ok_or(ElasticError::OutputFrameLimitOutOfRange(
                    self.max_output_frames,
                ))?;
        let max_source_frames =
            self.max_source_frames
                .to_f64()
                .ok_or(ElasticError::SourceFrameLimitOutOfRange(
                    self.max_source_frames,
                ))?;
        ElasticRateEnvelope::try_from((1.0 / max_output_frames)..=max_source_frames)
    }

    pub(crate) const fn channels(self) -> usize {
        self.channels
    }

    pub(crate) const fn max_output_frames(self) -> usize {
        self.max_output_frames
    }

    pub(crate) const fn max_source_frames(self) -> usize {
        self.max_source_frames
    }

    pub(crate) const fn sample_rate(self) -> u32 {
        self.sample_rate
    }
}

/// Backends address blocks and channels with `i32`, so every prepared count is
/// positive and representable there.
fn frame_count(
    value: usize,
    empty: ElasticError,
    out_of_range: fn(usize) -> ElasticError,
) -> Result<usize, ElasticError> {
    if value == 0 {
        return Err(empty);
    }
    i32::try_from(value).map_err(|_| out_of_range(value))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn span_config_requires_finite_positive_values() {
        for values in [
            (0.0, 1.0, 1.0),
            (1.0e-6, f64::NAN, 1.0),
            (1.0e-6, 1.0, f64::INFINITY),
        ] {
            assert!(matches!(
                ElasticSpanConfig::try_from(values),
                Err(ElasticError::InvalidSpanConfig { .. })
            ));
        }
    }

    #[kithara::test]
    fn span_config_preserves_valid_policy_values() {
        let config = ElasticSpanConfig::try_from((1.0e-5, 0.5, 0.25))
            .expect("invariant: finite positive span policy");

        assert_eq!(config.continuity_tolerance(), 1.0e-5);
        assert_eq!(config.max_phase_error(), 0.5);
        assert_eq!(config.max_correction_per_block(), 0.25);
    }

    #[kithara::test]
    fn config_names_the_shape_field_it_rejects() {
        let out_of_range = usize::try_from(i32::MAX).map_or(usize::MAX, |limit| limit + 1);

        for ((sample_rate, channels, max_source_frames, max_output_frames), expected) in [
            ((0, 2, 512, 512), ElasticError::InvalidSampleRate),
            ((48_000, 0, 512, 512), ElasticError::InvalidChannelCount),
            (
                (48_000, out_of_range, 512, 512),
                ElasticError::ChannelCountOutOfRange(out_of_range),
            ),
            ((48_000, 2, 0, 512), ElasticError::InvalidSourceFrameLimit),
            (
                (48_000, 2, out_of_range, 512),
                ElasticError::SourceFrameLimitOutOfRange(out_of_range),
            ),
            ((48_000, 2, 512, 0), ElasticError::InvalidOutputFrameLimit),
            (
                (48_000, 2, 512, out_of_range),
                ElasticError::OutputFrameLimitOutOfRange(out_of_range),
            ),
        ] {
            let actual = ElasticConfig::builder()
                .pool(PcmPool::default())
                .sample_rate(sample_rate)
                .channels(channels)
                .max_source_frames(max_source_frames)
                .max_output_frames(max_output_frames)
                .build();

            assert!(matches!(actual, Err(error) if error == expected));
        }
    }

    #[kithara::test]
    fn config_declares_the_complete_non_empty_frame_domain() {
        let config = ElasticConfig::builder()
            .pool(PcmPool::default())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(960)
            .max_output_frames(480)
            .build()
            .expect("valid elastic config");
        let envelope = config.shape().rate_envelope().expect("valid frame domain");

        assert_eq!(envelope.min_source_frames_per_output(), 1.0 / 480.0);
        assert_eq!(envelope.max_source_frames_per_output(), 960.0);
    }
}
