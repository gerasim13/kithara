use super::ElasticError;

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

/// Immutable engine preparation shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct ElasticConfig {
    /// Source sample rate in Hz.
    #[field(get, copy)]
    sample_rate: u32,
    /// Interleaved channel count.
    #[field(get, copy)]
    channels: usize,
    /// Largest accepted output block in frames.
    #[field(get, copy)]
    max_output_frames: usize,
    /// Largest accepted source block in frames.
    #[field(get, copy)]
    max_source_frames: usize,
}

impl TryFrom<(u32, usize, usize, usize)> for ElasticConfig {
    type Error = ElasticError;

    fn try_from(
        (sample_rate, channels, max_source_frames, max_output_frames): (u32, usize, usize, usize),
    ) -> Result<Self, Self::Error> {
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
        Ok(Self {
            sample_rate,
            channels,
            max_output_frames,
            max_source_frames,
        })
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

        for (shape, expected) in [
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
            assert_eq!(ElasticConfig::try_from(shape), Err(expected));
        }
    }
}
