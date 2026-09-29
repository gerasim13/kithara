use fearless_simd::{Level, dispatch, prelude::*};

use crate::backend::portable::dot;

/// `output[k] = Σ_j signal[k + j]·kernel[j]` for every lag `k` at which the
/// kernel fits inside `signal`, as many as `output` holds; returns the lag
/// count.
pub(crate) fn correlate(signal: &[f32], kernel: &[f32], output: &mut [f32]) -> usize {
    dispatch!(Level::new(), simd => correlate_kernel(simd, signal, kernel, output))
}

#[inline(always)]
pub(super) fn correlate_kernel<S: Simd>(
    simd: S,
    signal: &[f32],
    kernel: &[f32],
    output: &mut [f32],
) -> usize {
    let taps = kernel.len();
    if taps == 0 {
        return 0;
    }
    let lags = signal
        .len()
        .saturating_add(1)
        .saturating_sub(taps)
        .min(output.len());
    for (slot, window) in output.iter_mut().zip(signal.windows(taps)) {
        *slot = dot(simd, window, kernel);
    }
    lags
}
