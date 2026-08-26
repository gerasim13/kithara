use std::ops::RangeInclusive;

use super::{ElasticError, ElasticRequest};

/// Supported source-frame advance per output frame.
///
/// The envelope spans every ratio constructible from one non-empty exact
/// request inside the prepared source and output frame limits.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct ElasticRateEnvelope {
    /// Maximum source-frame advance per output frame.
    #[field(get, copy)]
    max_source_frames_per_output: f64,
    /// Minimum source-frame advance per output frame.
    #[field(get, copy)]
    min_source_frames_per_output: f64,
}

impl ElasticRateEnvelope {
    pub(crate) fn contains(self, request: ElasticRequest) -> bool {
        request
            .source_frames_per_output()
            .is_ok_and(|rate| self.contains_rate(rate))
    }

    /// Returns whether a continuous source advance is supported.
    #[must_use]
    pub fn contains_rate(self, source_frames_per_output: f64) -> bool {
        source_frames_per_output.is_finite()
            && source_frames_per_output >= self.min_source_frames_per_output.next_down()
            && source_frames_per_output <= self.max_source_frames_per_output.next_up()
    }
}

impl TryFrom<RangeInclusive<f64>> for ElasticRateEnvelope {
    type Error = ElasticError;

    fn try_from(rates: RangeInclusive<f64>) -> Result<Self, Self::Error> {
        let (min_source_frames_per_output, max_source_frames_per_output) = rates.into_inner();
        if [min_source_frames_per_output, max_source_frames_per_output]
            .into_iter()
            .any(|rate| !rate.is_finite() || rate <= 0.0)
            || min_source_frames_per_output > max_source_frames_per_output
        {
            return Err(ElasticError::InvalidRateEnvelope {
                max: max_source_frames_per_output,
                min: min_source_frames_per_output,
            });
        }
        Ok(Self {
            max_source_frames_per_output,
            min_source_frames_per_output,
        })
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{ElasticError, ElasticRateEnvelope};

    fn envelope() -> ElasticRateEnvelope {
        ElasticRateEnvelope::try_from(2.0 / 3.0..=4.0 / 3.0)
            .expect("invariant: the declared window is finite, positive and ordered")
    }

    #[kithara::test]
    fn accepts_one_rounding_step_at_the_declared_rate_boundary() {
        let envelope = envelope();
        let minimum = envelope.min_source_frames_per_output();
        let maximum = envelope.max_source_frames_per_output();
        let one_step_below = minimum.next_down();
        let two_steps_below = one_step_below.next_down();
        let one_step_above = maximum.next_up();
        let two_steps_above = one_step_above.next_up();

        assert!(envelope.contains_rate(one_step_below));
        assert!(!envelope.contains_rate(two_steps_below));
        assert!(envelope.contains_rate(one_step_above));
        assert!(!envelope.contains_rate(two_steps_above));
    }

    #[kithara::test]
    fn rejects_windows_that_cannot_bound_a_rate() {
        for rates in [
            0.0..=1.0,
            f64::NAN..=1.0,
            1.0..=f64::INFINITY,
            4.0 / 3.0..=2.0 / 3.0,
        ] {
            assert!(matches!(
                ElasticRateEnvelope::try_from(rates),
                Err(ElasticError::InvalidRateEnvelope { .. })
            ));
        }
    }
}
