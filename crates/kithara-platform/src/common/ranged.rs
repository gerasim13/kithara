/// Declare a newtype whose value is kept inside a fixed range.
///
/// The generated type stores one float, carries its own bounds as `MIN` /
/// `MAX` and its fallback as `DEFAULT` — all three values of the type itself —
/// and is built through `From`, which clamps, or `checked`, which rejects an
/// invalid value. A caller imports the type instead of a pair of loose bounds
/// constants, and cannot hold a value the range does not allow.
///
/// `NaN` is in no range, so it yields `DEFAULT` rather than propagating.
///
/// ```
/// kithara_platform::ranged!(
///     /// Fraction of the way through a fade.
///     pub struct Progress(f32, 0.0, 1.0, 0.0)
/// );
///
/// assert_eq!(Progress::from(2.0), Progress::MAX);
/// assert_eq!(Progress::default(), Progress::DEFAULT);
/// assert_eq!(f32::from(Progress::from(0.25)), 0.25);
/// ```
#[macro_export]
macro_rules! ranged {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident($t:ty, $min:expr, $max:expr, $default:expr)
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, PartialOrd)]
        $vis struct $name($t);

        impl $name {
            /// Lowest value this type can hold.
            pub const MIN: Self = Self($min);
            /// Highest value this type can hold.
            pub const MAX: Self = Self($max);
            /// Value a default-constructed instance carries, and the answer to
            /// a `NaN` input.
            pub const DEFAULT: Self = Self($default);

            /// Builds a value only when it is finite and inside the range.
            #[must_use]
            pub fn checked(value: $t) -> Option<Self> {
                if value.is_finite() && (Self::MIN.0..=Self::MAX.0).contains(&value) {
                    Some(Self(value))
                } else {
                    None
                }
            }

            fn clamp(value: $t) -> $t {
                if value.is_nan() {
                    Self::DEFAULT.0
                } else {
                    value.clamp(Self::MIN.0, Self::MAX.0)
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::DEFAULT
            }
        }

        impl From<$t> for $name {
            fn from(value: $t) -> Self {
                Self(Self::clamp(value))
            }
        }

        impl From<$name> for $t {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    crate::ranged!(
        /// Asymmetric on purpose: the two ends have to be read separately.
        pub struct Probe(f32, -24.0, 6.0, 0.0)
    );

    #[kithara::test]
    fn a_value_above_the_range_lands_on_the_ceiling() {
        assert_eq!(Probe::from(100.0), Probe::MAX);
    }

    #[kithara::test]
    fn a_value_below_the_range_lands_on_the_floor() {
        assert_eq!(Probe::from(-100.0), Probe::MIN);
    }

    #[kithara::test]
    fn a_value_inside_the_range_is_kept_exactly() {
        assert_eq!(f32::from(Probe::from(-3.5)), -3.5);
    }

    /// `NaN` compares false against both bounds, so clamping alone would carry
    /// it through and poison every comparison downstream of it.
    #[kithara::test]
    fn a_nan_becomes_the_default() {
        assert_eq!(Probe::from(f32::NAN), Probe::DEFAULT);
    }

    #[kithara::test]
    fn the_default_value_is_the_declared_one() {
        assert_eq!(Probe::default(), Probe::DEFAULT);
    }

    #[kithara::test]
    fn checked_construction_keeps_an_in_range_value() {
        assert_eq!(Probe::checked(-3.5), Some(Probe::from(-3.5)));
    }

    #[kithara::test]
    #[case::below(-24.1)]
    #[case::above(6.1)]
    #[case::not_a_number(f32::NAN)]
    #[case::positive_infinity(f32::INFINITY)]
    #[case::negative_infinity(f32::NEG_INFINITY)]
    fn checked_construction_rejects_an_invalid_value(#[case] value: f32) {
        assert_eq!(Probe::checked(value), None);
    }
}
