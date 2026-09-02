use std::ops::RangeInclusive;

use bon::bon;
use kithara_bufpool::PoolRegion;
use num_traits::{Float, ToPrimitive};

use super::{ElasticError, ElasticRateEnvelope};
use crate::StretchKind;

struct Consts;

impl Consts {
    const CONTINUITY_TOLERANCE: f64 = 1.0e-6;
    const MAX_CORRECTION_PER_BLOCK: f64 = 1.0;
    const MAX_PHASE_ERROR: f64 = 1.0;
    /// i32-bounded numerators and denominators need fewer than 47 continued-fraction steps.
    const RATE_FRACTION_DEPTH: u8 = 64;
    const MAX_SOURCE_FRAMES_PER_OUTPUT: f64 = 4.0;
    const MIN_SOURCE_FRAMES_PER_OUTPUT: f64 = 0.05;
}

/// Numeric continuity policy for exact-span planning.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, copy)]
#[non_exhaustive]
pub struct ElasticSpanConfig {
    /// Source-frame tolerance for adjacent spans; defaults to `1e-6`.
    continuity_tolerance: f64,
    /// Per-block source-frame correction limit; defaults to one frame.
    max_correction_per_block: f64,
    /// Accepted boundary phase error; defaults to one source frame.
    max_phase_error: f64,
}

#[bon]
impl ElasticSpanConfig {
    #[builder(
        builder_type(vis = "pub"),
        start_fn(name = builder, vis = "pub"),
        finish_fn(vis = "pub")
    )]
    fn new(
        #[builder(default = Consts::CONTINUITY_TOLERANCE)] continuity_tolerance: f64,
        #[builder(default = Consts::MAX_PHASE_ERROR)] max_phase_error: f64,
        #[builder(default = Consts::MAX_CORRECTION_PER_BLOCK)] max_correction_per_block: f64,
    ) -> Result<Self, ElasticError> {
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
pub struct ElasticConfig<S> {
    /// Selected compiled implementation.
    #[field(get(copy))]
    backend: StretchKind,
    /// Shared pool region used by engines that need planar scratch.
    #[field(get)]
    pools: PoolRegion<S>,
    #[field(get(copy), vis = "pub(crate)")]
    shape: ElasticShape,
}

#[bon]
impl<S> ElasticConfig<S> {
    /// Builds a validated preparation config with its shared pool region.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when a scalar is zero, the requested rate
    /// policy has no representable request, or a value cannot be represented
    /// by the native engines.
    #[builder(
        builder_type(vis = "pub"),
        start_fn(name = builder, vis = "pub"),
        finish_fn(vis = "pub")
    )]
    fn new(
        #[builder(default)] backend: StretchKind,
        pools: PoolRegion<S>,
        sample_rate: u32,
        channels: usize,
        max_source_frames: usize,
        max_output_frames: usize,
        #[builder(
            default = Consts::MIN_SOURCE_FRAMES_PER_OUTPUT
                ..=Consts::MAX_SOURCE_FRAMES_PER_OUTPUT
        )]
        rate_envelope: RangeInclusive<f64>,
    ) -> Result<Self, ElasticError> {
        if sample_rate == 0 {
            return Err(ElasticError::InvalidSampleRate);
        }
        let channels = channel_count(channels)?;
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
        let shape = ElasticShape::new(
            channels,
            max_output_frames,
            max_source_frames,
            sample_rate,
            ElasticRateEnvelope::try_from(rate_envelope)?,
        )?;
        Ok(Self {
            backend,
            pools,
            shape,
        })
    }

    delegate::delegate! {
        to self.shape {
            /// Prepared interleaved channel count.
            #[must_use]
            pub fn channels(&self) -> usize;
            /// Largest accepted output block in frames.
            #[must_use]
            pub fn max_output_frames(&self) -> usize;
            /// Largest accepted source block in frames.
            #[must_use]
            pub fn max_source_frames(&self) -> usize;
            /// Prepared source sample rate in Hz.
            #[must_use]
            pub fn sample_rate(&self) -> u32;
            /// Effective source-frame advance range after preparation.
            #[must_use]
            pub fn rate_envelope(&self) -> ElasticRateEnvelope;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, copy, vis = "pub(crate)")]
pub(crate) struct ElasticShape {
    channels: usize,
    max_output_frames: usize,
    max_source_frames: usize,
    #[field(get(copy))]
    rate_envelope: ElasticRateEnvelope,
    sample_rate: u32,
}

impl ElasticShape {
    fn new(
        channels: usize,
        max_output_frames: usize,
        max_source_frames: usize,
        sample_rate: u32,
        configured_rate_envelope: ElasticRateEnvelope,
    ) -> Result<Self, ElasticError> {
        let max_output_frames_f64 = max_output_frames
            .to_f64()
            .ok_or(ElasticError::OutputFrameLimitOutOfRange(max_output_frames))?;
        let max_source_frames_f64 = max_source_frames
            .to_f64()
            .ok_or(ElasticError::SourceFrameLimitOutOfRange(max_source_frames))?;
        let min_rate = configured_rate_envelope
            .min_source_frames_per_output()
            .max(Consts::MIN_SOURCE_FRAMES_PER_OUTPUT)
            .max(1.0 / max_output_frames_f64);
        let max_rate = configured_rate_envelope
            .max_source_frames_per_output()
            .min(Consts::MAX_SOURCE_FRAMES_PER_OUTPUT)
            .min(max_source_frames_f64);
        let rate_envelope = ElasticRateEnvelope::try_from(min_rate..=max_rate)?;
        if !has_representable_request(rate_envelope, max_source_frames, max_output_frames) {
            return Err(ElasticError::InvalidRateEnvelope {
                max: max_rate,
                min: min_rate,
            });
        }

        Ok(Self {
            channels,
            max_output_frames,
            max_source_frames,
            rate_envelope,
            sample_rate,
        })
    }
}

fn has_representable_request(
    envelope: ElasticRateEnvelope,
    max_source_frames: usize,
    max_output_frames: usize,
) -> bool {
    let accepted_minimum = envelope.min_source_frames_per_output().next_down();
    let accepted_maximum = envelope.max_source_frames_per_output().next_up();
    // Convert the accepted f64 edges into their exact division-rounding basins.
    let Some(minimum) = binary_midpoint(accepted_minimum.next_down(), accepted_minimum) else {
        return false;
    };
    let Some(maximum) = binary_midpoint(accepted_maximum, accepted_maximum.next_up()) else {
        return false;
    };
    let Some((_, denominator)) = simplest_fraction(minimum, maximum, Consts::RATE_FRACTION_DEPTH)
    else {
        return false;
    };
    let Ok(max_output_frames) = u128::try_from(max_output_frames) else {
        return false;
    };
    if denominator > max_output_frames {
        return false;
    }
    let Some(scaled_minimum) = minimum.0.checked_mul(denominator) else {
        return false;
    };
    let source_frames =
        scaled_minimum / minimum.1 + u128::from(!scaled_minimum.is_multiple_of(minimum.1));
    let Some(scaled_source) = source_frames.checked_mul(maximum.1) else {
        return false;
    };
    let Some(scaled_maximum) = maximum.0.checked_mul(denominator) else {
        return false;
    };
    let Ok(max_source_frames) = u128::try_from(max_source_frames) else {
        return false;
    };
    source_frames > 0 && source_frames <= max_source_frames && scaled_source <= scaled_maximum
}

fn binary_midpoint(left: f64, right: f64) -> Option<(u128, u128)> {
    let left = binary_fraction(left)?;
    let right = binary_fraction(right)?;
    let denominator = left.1.max(right.1);
    let numerator = left
        .0
        .checked_mul(denominator / left.1)?
        .checked_add(right.0.checked_mul(denominator / right.1)?)?;
    Some((numerator, denominator.checked_mul(2)?))
}

fn binary_fraction(value: f64) -> Option<(u128, u128)> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let (mantissa, exponent, sign) = value.integer_decode();
    if sign <= 0 {
        return None;
    }
    let mantissa = u128::from(mantissa);
    if exponent >= 0 {
        Some((mantissa.checked_shl(u32::try_from(exponent).ok()?)?, 1))
    } else {
        Some((
            mantissa,
            1_u128.checked_shl(u32::from(exponent.unsigned_abs()))?,
        ))
    }
}

fn simplest_fraction(
    minimum: (u128, u128),
    maximum: (u128, u128),
    depth: u8,
) -> Option<(u128, u128)> {
    if depth == 0 {
        return None;
    }
    let whole = minimum.0 / minimum.1;
    let maximum_whole = maximum.0 / maximum.1;
    if whole < maximum_whole {
        return Some((whole.checked_add(1)?, 1));
    }
    let minimum_remainder = minimum.0 % minimum.1;
    if minimum_remainder == 0 {
        return Some((whole, 1));
    }
    let maximum_remainder = maximum.0 % maximum.1;
    let (numerator, denominator) = simplest_fraction(
        (maximum.1, maximum_remainder),
        (minimum.1, minimum_remainder),
        depth - 1,
    )?;
    Some((
        whole.checked_mul(numerator)?.checked_add(denominator)?,
        numerator,
    ))
}

/// [`kithara_signal::AudioSpec`] represents channel counts with `u16`.
fn channel_count(value: usize) -> Result<usize, ElasticError> {
    if value == 0 {
        return Err(ElasticError::InvalidChannelCount);
    }
    u16::try_from(value).map_err(|_| ElasticError::ChannelCountOutOfRange(value))?;
    Ok(value)
}

/// Backends address frame blocks with `i32`, so every prepared count is
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
    use crate::test_pools::pools;

    #[kithara::test]
    fn span_config_requires_finite_positive_values() {
        for (continuity_tolerance, max_phase_error, max_correction_per_block) in [
            (0.0, 1.0, 1.0),
            (1.0e-6, f64::NAN, 1.0),
            (1.0e-6, 1.0, f64::INFINITY),
        ] {
            let result = ElasticSpanConfig::builder()
                .continuity_tolerance(continuity_tolerance)
                .max_phase_error(max_phase_error)
                .max_correction_per_block(max_correction_per_block)
                .build();
            assert!(matches!(
                result,
                Err(ElasticError::InvalidSpanConfig { .. })
            ));
        }
    }

    #[kithara::test]
    fn span_config_uses_runtime_policy_defaults() {
        let config = ElasticSpanConfig::builder()
            .build()
            .expect("invariant: defaults form a valid span policy");

        assert_eq!(config.continuity_tolerance(), 1.0e-6);
        assert_eq!(config.max_phase_error(), 1.0);
        assert_eq!(config.max_correction_per_block(), 1.0);
    }

    #[kithara::test]
    fn span_config_preserves_valid_policy_values() {
        let config = ElasticSpanConfig::builder()
            .continuity_tolerance(1.0e-5)
            .max_phase_error(0.5)
            .max_correction_per_block(0.25)
            .build()
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
                .pools(pools())
                .sample_rate(sample_rate)
                .channels(channels)
                .max_source_frames(max_source_frames)
                .max_output_frames(max_output_frames)
                .build();

            assert!(matches!(actual, Err(error) if error == expected));
        }
    }

    #[kithara::test]
    fn config_defaults_to_the_common_practical_rate_envelope() {
        let config = ElasticConfig::builder()
            .pools(pools())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(960)
            .max_output_frames(480)
            .build()
            .expect("valid elastic config");
        let envelope = config.rate_envelope();

        assert_eq!(config.backend(), StretchKind::default());
        assert_eq!(envelope.min_source_frames_per_output(), 0.05);
        assert_eq!(envelope.max_source_frames_per_output(), 4.0);
    }

    #[kithara::test]
    fn config_intersects_the_rate_policy_with_common_and_prepared_limits() {
        let config = ElasticConfig::builder()
            .pools(pools())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(2)
            .max_output_frames(40)
            .rate_envelope(0.01..=8.0)
            .build()
            .expect("valid elastic config");
        let envelope = config.rate_envelope();

        assert_eq!(envelope.min_source_frames_per_output(), 0.05);
        assert_eq!(envelope.max_source_frames_per_output(), 2.0);
    }

    #[kithara::test]
    fn config_rejects_a_rate_envelope_without_a_representable_request() {
        let result = ElasticConfig::builder()
            .pools(pools())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(4)
            .max_output_frames(4)
            .rate_envelope(5.0..=6.0)
            .build();

        assert!(matches!(
            result,
            Err(ElasticError::InvalidRateEnvelope { min: 5.0, max: 4.0 })
        ));
    }

    #[kithara::test]
    fn config_rejects_a_continuous_window_without_a_discrete_request() {
        let result = ElasticConfig::builder()
            .pools(pools())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(1)
            .max_output_frames(100)
            .rate_envelope(0.051..=0.052)
            .build();

        assert!(matches!(
            result,
            Err(ElasticError::InvalidRateEnvelope {
                min: 0.051,
                max: 0.052
            })
        ));
    }

    #[kithara::test]
    fn config_preserves_a_continuous_window_with_a_discrete_request() {
        let config = ElasticConfig::builder()
            .pools(pools())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(2)
            .max_output_frames(100)
            .rate_envelope(0.051..=0.052)
            .build()
            .expect("2/39 is representable inside the configured window");

        assert_eq!(
            config.rate_envelope(),
            ElasticRateEnvelope::try_from(0.051..=0.052)
                .expect("invariant: finite positive ordered envelope")
        );
    }

    #[kithara::test]
    fn config_accepts_a_request_on_the_tolerated_ulp_boundary() {
        let boundary = 0.75_f64.next_up();
        let config = ElasticConfig::builder()
            .pools(pools())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(3)
            .max_output_frames(4)
            .rate_envelope(boundary..=boundary)
            .build()
            .expect("3/4 is one rounding step below the declared boundary");

        assert!(config.rate_envelope().contains_rate(0.75));
    }

    #[kithara::test]
    fn config_accounts_for_rounding_the_request_rate() {
        let config = ElasticConfig::builder()
            .pools(pools())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(2)
            .max_output_frames(3)
            .rate_envelope(0.625_000_000_000_000_1..=0.666_666_666_666_666_5)
            .build()
            .expect("2/3 rounds to the accepted upper boundary");

        assert!(config.rate_envelope().contains_rate(2.0 / 3.0));
    }
}
