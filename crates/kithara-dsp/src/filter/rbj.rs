use std::f64::consts::TAU;

use super::{Coefficients, FilterError};

/// Low-pass section with corner `cutoff` Hz and quality `q` at `sample_rate` Hz.
///
/// # Errors
/// [`FilterError::Parameters`] when a value is not finite and positive, the
/// corner reaches Nyquist, or the section is unstable in `f32`.
pub fn low_pass(sample_rate: f64, cutoff: f64, q: f64) -> Result<Coefficients, FilterError> {
    let positive = [sample_rate, cutoff, q]
        .iter()
        .all(|value| value.is_finite() && *value > 0.0);
    if !positive || cutoff >= sample_rate * 0.5 {
        return Err(FilterError::Parameters);
    }
    let (sin, cos) = (TAU * cutoff / sample_rate).sin_cos();
    let alpha = sin / (2.0 * q);
    let a0 = 1.0 + alpha;
    let b1 = (1.0 - cos) / a0;
    let b0 = b1 * 0.5;
    Coefficients::new([b0, b1, b0, -2.0 * cos / a0, (1.0 - alpha) / a0])
}
