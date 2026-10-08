use std::{
    num::{NonZeroU32, NonZeroU64, NonZeroU128},
    ops::Range,
};

use kithara_platform::time::Duration;

/// Decoded-source interval represented by a physical output interval.
///
/// Slices retain the exact rational source position and slope. Integer source
/// endpoints are rounded down only when queried, never used as a new basis.
/// Coefficients retain 128-bit precision when a curve and correction share a basis.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct SourceSpan {
    #[field(get, copy)]
    sample_rate: NonZeroU32,
    denominator: NonZeroU128,
    #[field(get, copy, with)]
    mapping_revision: Option<NonZeroU64>,
    numerator: u128,
    #[field(get, copy)]
    output_frames: u64,
    #[field(get, copy, with)]
    render_revision: u64,
    step: u128,
    step_change: i128,
}

fn fractional_nanos(numerator: u128, denominator: u128) -> u32 {
    let mut remainder = 0;
    let mut nanos = 0;
    for bit in (0..u32::BITS).rev() {
        let complement = denominator - remainder;
        let mut carry = if remainder >= complement {
            remainder -= complement;
            1
        } else {
            remainder += remainder;
            0
        };
        if 1_000_000_000u32 & (1 << bit) != 0 {
            let complement = denominator - numerator;
            if remainder >= complement {
                remainder -= complement;
                carry += 1;
            } else {
                remainder += numerator;
            }
        }
        nanos = nanos * 2 + carry;
    }
    nanos
}

impl SourceSpan {
    /// Source position at an output boundary, retaining rational phase.
    #[must_use]
    pub fn position_at(self, output_frame: u64) -> Option<Duration> {
        let (numerator, denominator) = self.source_ratio_at(output_frame)?;
        let frames = numerator / denominator.get();
        let rate = u128::from(self.sample_rate.get());
        let seconds = u64::try_from(frames / rate).ok()?;
        let fraction = fractional_nanos(numerator % denominator.get(), denominator.get());
        let nanos =
            u32::try_from(((frames % rate) * 1_000_000_000 + u128::from(fraction)) / rate).ok()?;
        Some(Duration::new(seconds, nanos))
    }

    /// Creates a mapping for a nonempty physical output interval.
    #[must_use]
    pub fn new(start: u64, end: u64, sample_rate: NonZeroU32, output_frames: u64) -> Option<Self> {
        let source_frames = end.checked_sub(start)?;
        let mut divisor = source_frames;
        let mut remainder = output_frames;
        while remainder != 0 {
            (divisor, remainder) = (remainder, divisor % remainder);
        }
        let denominator = NonZeroU128::new(u128::from(output_frames.checked_div(divisor)?))?;
        Self::from_rational(
            u128::from(start) * denominator.get(),
            u128::from(source_frames / divisor),
            denominator,
            sample_rate,
            output_frames,
        )
    }

    /// Creates a checked affine mapping with a fractional source origin.
    #[must_use]
    pub fn from_rational(
        start_numerator: u128,
        step_numerator: u128,
        denominator: NonZeroU128,
        sample_rate: NonZeroU32,
        output_frames: u64,
    ) -> Option<Self> {
        Self::from_ramp(
            start_numerator,
            step_numerator,
            0,
            denominator,
            sample_rate,
            output_frames,
        )
    }

    /// Creates a checked mapping whose frame advances form an arithmetic progression.
    /// Boundary `n` is `(start + step*n + step_change*n*(n-1)/2) / denominator`.
    #[must_use]
    pub fn from_ramp(
        start_numerator: u128,
        step_numerator: u128,
        step_change: i128,
        denominator: NonZeroU128,
        sample_rate: NonZeroU32,
        output_frames: u64,
    ) -> Option<Self> {
        if output_frames == 0 {
            return None;
        }
        let span = Self {
            numerator: start_numerator,
            step: step_numerator,
            step_change,
            denominator,
            output_frames,
            sample_rate,
            render_revision: 0,
            mapping_revision: None,
        };
        span.step_at(output_frames - 1)?;
        let end = span.numerator_at(output_frames)?;
        u64::try_from(end / denominator.get()).ok()?;
        Some(span)
    }

    fn step_at(self, frame: u64) -> Option<u128> {
        let change = self
            .step_change
            .unsigned_abs()
            .checked_mul(u128::from(frame))?;
        if self.step_change < 0 {
            self.step.checked_sub(change)
        } else {
            self.step.checked_add(change)
        }
    }

    fn numerator_at(self, frame: u64) -> Option<u128> {
        let linear = self.step.checked_mul(u128::from(frame))?;
        let pairs = u128::from(frame).checked_mul(u128::from(frame.saturating_sub(1)))? / 2;
        let change = pairs.checked_mul(self.step_change.unsigned_abs())?;
        let advance = if self.step_change < 0 {
            linear.checked_sub(change)?
        } else {
            linear.checked_add(change)?
        };
        self.numerator.checked_add(advance)
    }

    /// Exact reduced decoded-source coordinate at an output boundary.
    #[must_use]
    pub fn source_ratio_at(self, output_frame: u64) -> Option<(u128, NonZeroU128)> {
        if output_frame > self.output_frames {
            return None;
        }
        let numerator = self.numerator_at(output_frame)?;
        let mut divisor = self.denominator.get();
        let mut remainder = numerator;
        while remainder != 0 {
            (divisor, remainder) = (remainder, divisor % remainder);
        }
        Some((
            numerator / divisor,
            NonZeroU128::new(self.denominator.get() / divisor)?,
        ))
    }

    /// Exclusive decoded-source frame, rounded down on the source lattice.
    ///
    /// # Panics
    /// Panics if the private validated mapping invariant is violated.
    #[must_use]
    pub fn end(self) -> u64 {
        let Some(end) = self
            .numerator_at(self.output_frames)
            .and_then(|numerator| u64::try_from(numerator / self.denominator.get()).ok())
        else {
            unreachable!("validated source mapping endpoint");
        };
        end
    }

    /// Joins adjacent output intervals only when their exact mappings agree.
    #[must_use]
    pub fn followed_by(self, next: Self) -> Option<Self> {
        let boundary = self.numerator_at(self.output_frames)?;
        if self.step_at(self.output_frames)? != next.step
            || self.step_change != next.step_change
            || self.denominator != next.denominator
            || boundary != next.numerator
            || self.sample_rate != next.sample_rate
            || self.render_revision != next.render_revision
            || self.mapping_revision != next.mapping_revision
        {
            return None;
        }
        let output_frames = self.output_frames.checked_add(next.output_frames)?;
        u64::try_from(self.numerator_at(output_frames)? / self.denominator.get()).ok()?;
        if output_frames > 0 {
            self.step_at(output_frames - 1)?;
        }
        Some(Self {
            output_frames,
            ..self
        })
    }

    /// Slices relative output frames without rounding the retained source basis.
    #[must_use]
    pub fn for_output_range(self, range: Range<u64>) -> Option<Self> {
        if range.start > range.end || range.end > self.output_frames {
            return None;
        }
        Some(Self {
            numerator: self.numerator_at(range.start)?,
            step: if range.start == range.end {
                0
            } else {
                self.step_at(range.start)?
            },
            output_frames: range.end - range.start,
            ..self
        })
    }

    /// Inclusive decoded-source frame, rounded down on the source lattice.
    ///
    /// # Panics
    /// Panics if the private validated mapping invariant is violated.
    ///
    /// Constructors accept `u64` endpoints, slicing stays within them, and joining requires the
    /// exact boundary of another validated interval — together they uphold the private mapping
    /// invariant this panics on.
    #[must_use]
    pub fn start(self) -> u64 {
        let Ok(start) = u64::try_from(self.numerator / self.denominator.get()) else {
            unreachable!("validated source mapping origin");
        };
        start
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn empty_source_mapping_slices_join_without_underflow() {
        let span = SourceSpan::from_rational(
            1,
            2,
            NonZeroU128::new(2).expect("denominator"),
            NonZeroU32::new(48_000).expect("rate"),
            8,
        )
        .expect("span");
        let empty = span.for_output_range(3..3).expect("empty slice");
        assert_eq!(empty.followed_by(empty), Some(empty));
        assert_eq!(empty.position_at(0), span.position_at(3));
    }

    #[kithara::test]
    fn fractional_start_survives_a_change_to_an_integer_slope() {
        let rate = NonZeroU32::new(48_000).expect("rate");
        let span =
            SourceSpan::from_rational(1, 2, NonZeroU128::new(2).expect("denominator"), rate, 8)
                .expect("fractional source origin");
        assert_eq!(span.position_at(0), Some(Duration::from_nanos(10_416)));
        assert_eq!(span.position_at(3), Some(Duration::from_nanos(72_916)));
        assert_eq!(
            span.source_ratio_at(3),
            Some((7, NonZeroU128::new(2).expect("denominator")))
        );
        assert_eq!(
            span.for_output_range(3..8).expect("suffix").position_at(0),
            span.position_at(3)
        );
    }

    #[kithara::test]
    fn rational_source_mapping_checks_its_entire_range() {
        let rate = NonZeroU32::new(48_000).expect("rate");
        assert!(SourceSpan::from_rational(u128::MAX, 1, NonZeroU128::MIN, rate, 1).is_none());
        assert!(
            SourceSpan::from_rational(u128::from(u64::MAX), 1, NonZeroU128::MIN, rate, 1).is_none()
        );
        assert!(SourceSpan::from_rational(0, 1, NonZeroU128::MIN, rate, 0).is_none());
        let standing =
            SourceSpan::from_rational(1, 0, NonZeroU128::new(2).expect("denominator"), rate, 8)
                .expect("standing fractional position");
        assert_eq!(standing.position_at(8), standing.position_at(0));
    }

    #[kithara::test]
    fn ramp_source_mapping_slices_the_analytic_integral() {
        let rate = NonZeroU32::new(48_000).expect("rate");
        let span = SourceSpan::from_ramp(
            0,
            33,
            2,
            NonZeroU128::new(64).expect("denominator"),
            rate,
            32,
        )
        .expect("positive ramp");
        assert_eq!(span.source_ratio_at(32), Some((32, NonZeroU128::MIN)));
        let first = span.for_output_range(0..7).expect("prefix");
        let second = span.for_output_range(7..32).expect("suffix");
        assert_eq!(second.position_at(0), span.position_at(7));
        assert_eq!(first.followed_by(second), Some(span));
        assert!(SourceSpan::from_ramp(0, 1, -2, NonZeroU128::MIN, rate, 2).is_none());
    }

    #[kithara::test]
    fn wide_source_mapping_keeps_exact_positions_slices_and_timestamps() {
        let rate = NonZeroU32::new(192_000).expect("rate");
        let denominator = NonZeroU128::new(1u128 << 90).expect("wide denominator");
        let origin = 6_912_000_000u128;
        let span = SourceSpan::from_ramp(
            origin * denominator.get() + 1,
            denominator.get(),
            1,
            denominator,
            rate,
            32,
        )
        .expect("wide mapping");
        assert_eq!(
            span.source_ratio_at(0),
            Some((origin * denominator.get() + 1, denominator))
        );
        assert_eq!(span.position_at(0), Some(Duration::from_secs(36_000)));
        assert_eq!(span.position_at(1), Some(Duration::new(36_000, 5_208)));
        let prefix = span.for_output_range(0..7).expect("prefix");
        let suffix = span.for_output_range(7..32).expect("suffix");
        assert_eq!(suffix.source_ratio_at(0), span.source_ratio_at(7));
        assert_eq!(prefix.followed_by(suffix), Some(span));
    }

    #[kithara::test]
    fn output_positions_retain_fractional_source_phase() {
        let rate = NonZeroU32::new(48_000).expect("rate");
        let span = SourceSpan::new(100, 292, rate, 128).expect("span");
        let sliced = span.for_output_range(1..127).expect("slice");
        assert_eq!(span.position_at(1), Some(Duration::from_nanos(2_114_583)));
        assert_eq!(sliced.position_at(1), span.position_at(2));
        assert_eq!(span.position_at(129), None);
        let large = SourceSpan::new(u64::MAX - 192, u64::MAX, rate, 128).expect("span");
        assert!(large.position_at(127).is_some());
    }

    #[kithara::test]
    fn nested_source_slices_keep_the_original_rational_phase() {
        let rate = NonZeroU32::new(48_000).expect("rate");
        for origin in [0, 100, u64::MAX - 192] {
            let span = SourceSpan::new(origin, origin + 192, rate, 128).expect("span");
            let nested = span
                .for_output_range(0..127)
                .expect("prefix")
                .for_output_range(0..2)
                .expect("nested prefix");
            assert_eq!(nested.end(), origin + 3);
            assert_eq!(Some(nested), span.for_output_range(0..2));
            let first = span.for_output_range(0..1).expect("first");
            let rest = span.for_output_range(1..128).expect("rest");
            assert_eq!(first.followed_by(rest), Some(span));
        }
    }

    #[kithara::test]
    fn rounded_endpoints_do_not_authorize_a_different_mapping_join() {
        let rate = NonZeroU32::new(48_000).expect("rate");
        let original = SourceSpan::new(0, 192, rate, 128).expect("span");
        let first = original.for_output_range(0..1).expect("first");
        let rounded = SourceSpan::new(1, 4, rate, 2).expect("same slope, wrong phase");
        assert!(first.followed_by(rounded).is_none());
    }
    #[kithara::test]
    fn source_span_requires_output_and_preserves_valid_source_boundaries() {
        let rate = NonZeroU32::new(48_000).expect("rate");
        assert!(SourceSpan::new(0, 0, rate, 0).is_none());
        assert!(SourceSpan::new(0, 1, rate, 0).is_none());
        assert!(SourceSpan::new(2, 1, rate, 1).is_none());
        let standing = SourceSpan::new(u64::MAX, u64::MAX, rate, 128).expect("standing source");
        assert_eq!(standing.start(), u64::MAX);
        assert_eq!(standing.end(), u64::MAX);
        assert_eq!(
            standing.for_output_range(127..128).expect("suffix").end(),
            u64::MAX
        );
        assert!(standing.for_output_range(0..129).is_none());
        assert!(
            standing
                .for_output_range(Range { start: 2, end: 1 })
                .is_none()
        );
        let full = SourceSpan::new(0, u64::MAX, rate, u64::MAX).expect("full source");
        assert_eq!(full.end(), u64::MAX);
        assert_eq!(
            full.for_output_range(1..u64::MAX).expect("suffix").start(),
            1
        );
    }
}
