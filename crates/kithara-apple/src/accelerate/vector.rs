use std::ptr;

use super::ffi::vDSP_maxmgv;

/// The largest `|x|` in `samples`; `0.0` for an empty slice.
#[must_use]
pub fn max_magnitude_f32(samples: &[f32]) -> f32 {
    let mut peak = 0.0;
    if samples.is_empty() {
        return peak;
    }
    // SAFETY: samples points at samples.len() contiguous f32 values with stride one.
    // SAFETY: peak is a valid f32 destination for the synchronous call.
    unsafe { vDSP_maxmgv(samples.as_ptr(), 1, ptr::from_mut(&mut peak), samples.len()) };
    peak
}
