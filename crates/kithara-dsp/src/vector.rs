use crate::backend::platform;

/// `Σx²` over `samples`; `0.0` for an empty slice.
///
/// Within `N·ε·Σx²` of the exact sum. A `NaN` or an infinite sample carries
/// through: the kernel never sanitizes.
#[must_use]
pub fn sum_squares(samples: &[f32]) -> f32 {
    platform::sum_squares(samples)
}
