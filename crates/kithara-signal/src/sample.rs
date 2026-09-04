/// Make a decoded sample safe downstream.
///
/// Non-finite values and denormals become silence.
#[must_use]
#[inline]
pub fn sanitize_sample(sample: f32) -> f32 {
    if !sample.is_finite() || sample.abs() < f32::MIN_POSITIVE {
        return 0.0;
    }
    sample
}
