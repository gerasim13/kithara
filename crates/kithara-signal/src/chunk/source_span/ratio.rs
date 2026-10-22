use std::num::NonZeroU128;

use num_traits::Float;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Ratio {
    pub(super) numerator: u128,
    pub(super) denominator: NonZeroU128,
}

pub(super) fn divisor(mut first: u128, mut second: u128) -> u128 {
    if second.is_power_of_two() {
        return 1u128 << first.trailing_zeros().min(second.trailing_zeros());
    }
    while second != 0 {
        (first, second) = (second, first % second);
    }
    first
}

fn quotient(numerator: u128, denominator: NonZeroU128) -> u128 {
    if denominator.get().is_power_of_two() {
        numerator >> denominator.get().trailing_zeros()
    } else {
        numerator / denominator.get()
    }
}

impl Ratio {
    pub(super) fn new(numerator: u128, denominator: NonZeroU128) -> Option<Self> {
        let common = NonZeroU128::new(divisor(numerator, denominator.get()))?;
        Some(Self {
            numerator: quotient(numerator, common),
            denominator: NonZeroU128::new(quotient(denominator.get(), common))?,
        })
    }

    pub(super) fn at(self, denominator: NonZeroU128) -> Option<u128> {
        self.numerator
            .checked_mul(quotient(denominator.get(), self.denominator))
    }

    pub(super) fn common(self, other: Self) -> Option<NonZeroU128> {
        let common = NonZeroU128::new(divisor(self.denominator.get(), other.denominator.get()))?;
        NonZeroU128::new(
            quotient(self.denominator.get(), common).checked_mul(other.denominator.get())?,
        )
    }

    pub(super) fn add(self, other: Self) -> Option<Self> {
        let denominator = self.common(other)?;
        Self::new(
            self.at(denominator)?.checked_add(other.at(denominator)?)?,
            denominator,
        )
    }

    pub(super) fn sub(self, other: Self) -> Option<Self> {
        let denominator = self.common(other)?;
        Self::new(
            self.at(denominator)?.checked_sub(other.at(denominator)?)?,
            denominator,
        )
    }

    pub(super) fn mul(self, other: Self) -> Option<Self> {
        let first = NonZeroU128::new(divisor(self.numerator, other.denominator.get()))?;
        let second = NonZeroU128::new(divisor(other.numerator, self.denominator.get()))?;
        Self::new(
            quotient(self.numerator, first).checked_mul(quotient(other.numerator, second))?,
            NonZeroU128::new(
                quotient(self.denominator.get(), second)
                    .checked_mul(quotient(other.denominator.get(), first))?,
            )?,
        )
    }

    pub(super) fn nearest_q32(self) -> Option<Self> {
        let denominator = self.denominator.get();
        let units = 1u128 << 32;
        let numerator = if denominator.is_power_of_two() {
            let bits = denominator.trailing_zeros();
            if bits <= 32 {
                self.numerator.checked_mul(1u128 << (32 - bits))?
            } else {
                let bits = bits - 32;
                let rounded = (self.numerator & ((1u128 << bits) - 1)) >= (1u128 << (bits - 1));
                (self.numerator >> bits).checked_add(u128::from(rounded))?
            }
        } else {
            let whole = (self.numerator / denominator).checked_mul(units)?;
            let mut remainder = self.numerator % denominator;
            let mut fractional = 0;
            for _ in 0..32 {
                fractional *= 2;
                let complement = denominator - remainder;
                if remainder >= complement {
                    remainder -= complement;
                    fractional += 1;
                } else {
                    remainder *= 2;
                }
            }
            whole
                .checked_add(fractional)?
                .checked_add(u128::from(remainder >= denominator - remainder))?
        };
        Self::new(numerator, NonZeroU128::new(units)?)
    }

    pub(super) fn source_frame(self) -> Option<u64> {
        u64::try_from(quotient(self.numerator, self.denominator)).ok()
    }
}

impl TryFrom<f64> for Ratio {
    type Error = ();

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if !value.is_finite() || value < 0.0 {
            return Err(());
        }
        if value == 0.0 {
            return Self::new(0, NonZeroU128::MIN).ok_or(());
        }
        let (mantissa, exponent, _) = value.integer_decode();
        let mut numerator = u128::from(mantissa);
        let mut exponent = i32::from(exponent);
        if exponent < 0 {
            let reduction = numerator.trailing_zeros().min(exponent.unsigned_abs());
            numerator >>= reduction;
            exponent += i32::try_from(reduction).map_err(|_| ())?;
        }
        let power = 1u128.checked_shl(exponent.unsigned_abs()).ok_or(())?;
        let denominator = if exponent >= 0 {
            numerator = numerator.checked_mul(power).ok_or(())?;
            NonZeroU128::MIN
        } else {
            NonZeroU128::new(power).ok_or(())?
        };
        Ok(Self {
            numerator,
            denominator,
        })
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn source_quotients_match_full_width_integer_division() {
        let numerators = [0, 1, 3, u128::from(u64::MAX), 1 << 64, 1 << 127, u128::MAX];
        for bits in 0..u128::BITS {
            let denominator = NonZeroU128::new(1 << bits).expect("positive dyadic divisor");
            for numerator in numerators.into_iter().chain([
                denominator.get() - 1,
                denominator.get(),
                denominator.get() + 1,
            ]) {
                assert_eq!(
                    quotient(numerator, denominator),
                    numerator / denominator.get(),
                    "numerator={numerator}, denominator={denominator}",
                );
            }
        }
        for denominator in [3, 5, 44100, u128::from(u64::MAX), u128::MAX] {
            let denominator = NonZeroU128::new(denominator).expect("positive general divisor");
            for numerator in numerators {
                assert_eq!(
                    quotient(numerator, denominator),
                    numerator / denominator.get()
                );
            }
        }
    }
}
