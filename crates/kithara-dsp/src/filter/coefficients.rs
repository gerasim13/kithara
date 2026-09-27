use num_traits::ToPrimitive;

use super::FilterError;

/// One normalized section `[b0, b1, b2, a1, a2]` of
/// `H(z) = (b0 + b1 z⁻¹ + b2 z⁻²) / (1 + a1 z⁻¹ + a2 z⁻²)`, finite and
/// stable in `f32`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefficients([f32; 5]);

impl Coefficients {
    /// # Errors
    /// [`FilterError::Parameters`] when a coefficient is not finite in `f32`
    /// or a pole leaves the unit circle.
    pub(crate) fn new(section: [f64; 5]) -> Result<Self, FilterError> {
        let section = section.map(|value| value.to_f32().unwrap_or(f32::NAN));
        let [_, _, _, a1, a2] = section;
        let stable = a2.abs() < 1.0 && a1.abs() < 1.0 + a2;
        if stable && section.iter().all(|value| value.is_finite()) {
            Ok(Self(section))
        } else {
            Err(FilterError::Parameters)
        }
    }

    pub(crate) const fn section(self) -> [f32; 5] {
        self.0
    }
}
