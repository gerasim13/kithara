use std::{
    num::{NonZeroU32, NonZeroU64, NonZeroU128},
    ops::Range,
};

use kithara_platform::time::Duration;
use num_traits::ToPrimitive;

use super::{
    affine::Affine,
    approach::Approach,
    mapping::{Mapping, Rational},
    ratio::Ratio,
};
use crate::ExponentialApproach;

/// Decoded-source interval represented by a physical output interval.
///
/// Rational mappings retain exact coefficients. Exponential mappings retain
/// their original analytic basis and round advance to the nearest Q32 lattice.
/// Slices never replace either basis with rounded source endpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct SourceSpan {
    #[field(get, copy)]
    sample_rate: NonZeroU32,
    #[field(get, copy, with)]
    mapping_revision: Option<NonZeroU64>,
    mapping: Mapping,
    #[field(get, copy)]
    output_frames: u64,
    #[field(get, copy, with)]
    render_revision: u64,
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
    /// Source position in seconds at an output boundary, without nanosecond truncation.
    #[must_use]
    pub fn seconds_at(self, output_frame: u64) -> Option<f64> {
        let (numerator, denominator) = self.source_ratio_at(output_frame)?;
        Some(numerator.to_f64()? / denominator.get().to_f64()? / f64::from(self.sample_rate.get()))
    }

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

    /// Maps an original exponential source basis over a nonempty output span.
    ///
    /// Rates are source frames per output frame; elapsed and the time constant
    /// are output frames. The rational origin is at curve elapsed zero.
    /// Unrepresentable coordinates or an overflowing range return `None`.
    #[must_use]
    pub fn approaching(
        origin_numerator: u128,
        origin_denominator: NonZeroU128,
        curve: ExponentialApproach,
        elapsed: u64,
        sample_rate: NonZeroU32,
        output_frames: u64,
    ) -> Option<Self> {
        if output_frames == 0 {
            return None;
        }
        let mapping = Mapping::Approach(Approach {
            curve,
            elapsed,
            origin: Ratio::new(origin_numerator, origin_denominator)?,
            affine: Affine::identity()?,
        });
        mapping.validate(output_frames)?;
        Some(Self {
            mapping,
            output_frames,
            sample_rate,
            render_revision: 0,
            mapping_revision: None,
        })
    }

    /// Applies `position + scale * (source - pivot)` without changing the basis.
    ///
    /// All three coordinates are exact ratios. A positive scale is required;
    /// negative or unrepresentable source results return `None`.
    #[must_use]
    pub fn scaled_from(
        self,
        pivot: (u128, NonZeroU128),
        position: (u128, NonZeroU128),
        scale: (u128, NonZeroU128),
    ) -> Option<Self> {
        let affine = Affine::new(
            Ratio::new(pivot.0, pivot.1)?,
            Ratio::new(position.0, position.1)?,
            Ratio::new(scale.0, scale.1)?,
        )?;
        let mapping = self.mapping.transformed(affine)?;
        mapping.validate(self.output_frames)?;
        Some(Self { mapping, ..self })
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
        Self::try_from((
            u128::from(start) * denominator.get(),
            u128::from(source_frames / divisor),
            denominator,
            sample_rate,
            output_frames,
        ))
        .ok()
    }

    /// Reduced source coordinate, exact for rational maps and Q32 for approaches.
    #[must_use]
    pub fn source_ratio_at(self, output_frame: u64) -> Option<(u128, NonZeroU128)> {
        if output_frame > self.output_frames {
            return None;
        }
        let position = self.mapping.ratio_at(output_frame)?;
        Some((position.numerator, position.denominator))
    }

    /// Exclusive decoded-source frame, rounded down on the source lattice.
    ///
    /// # Panics
    /// Panics if the private validated mapping invariant is violated.
    #[must_use]
    pub fn end(self) -> u64 {
        let Some(end) = self
            .mapping
            .ratio_at(self.output_frames)
            .and_then(Ratio::source_frame)
        else {
            unreachable!("validated source mapping endpoint");
        };
        end
    }

    /// Joins adjacent output intervals only when their exact mappings agree.
    #[must_use]
    pub fn followed_by(self, next: Self) -> Option<Self> {
        if self.sample_rate != next.sample_rate
            || self.render_revision != next.render_revision
            || self.mapping_revision != next.mapping_revision
        {
            return None;
        }
        let mapping = self.mapping.followed_by(next.mapping, self.output_frames)?;
        let output_frames = self.output_frames.checked_add(next.output_frames)?;
        mapping.validate(output_frames)?;
        Some(Self {
            mapping,
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
        let output_frames = range.end - range.start;
        let mapping = self.mapping.sliced(range.start, output_frames)?;
        mapping.validate(output_frames)?;
        Some(Self {
            mapping,
            output_frames,
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
        let Some(start) = self.mapping.ratio_at(0).and_then(Ratio::source_frame) else {
            unreachable!("validated source mapping origin");
        };
        start
    }
}

impl TryFrom<(u128, u128, NonZeroU128, NonZeroU32, u64)> for SourceSpan {
    type Error = ();

    /// Creates a checked affine mapping with a fractional source origin.
    fn try_from(value: (u128, u128, NonZeroU128, NonZeroU32, u64)) -> Result<Self, Self::Error> {
        let (start_numerator, step_numerator, denominator, sample_rate, output_frames) = value;
        Self::try_from((
            start_numerator,
            step_numerator,
            0,
            denominator,
            sample_rate,
            output_frames,
        ))
    }
}

impl TryFrom<(u128, u128, i128, NonZeroU128, NonZeroU32, u64)> for SourceSpan {
    type Error = ();

    /// Creates a checked arithmetic progression of source-frame advances.
    fn try_from(
        value: (u128, u128, i128, NonZeroU128, NonZeroU32, u64),
    ) -> Result<Self, Self::Error> {
        let (start_numerator, step_numerator, step_change, denominator, sample_rate, output_frames) =
            value;
        if output_frames == 0 {
            return Err(());
        }
        let mapping = Mapping::Rational(Rational {
            numerator: start_numerator,
            step: step_numerator,
            step_change,
            denominator,
        });
        mapping.validate(output_frames).ok_or(())?;
        let span = Self {
            mapping,
            output_frames,
            sample_rate,
            render_revision: 0,
            mapping_revision: None,
        };
        Ok(span)
    }
}
