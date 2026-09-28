use std::ops::Mul;

use fearless_simd::{Level, dispatch, prelude::*};

use crate::backend::simd::zip_map;

/// `√(re² + im²)` per bin: within two epsilons of `hypot` while the squares
/// stay finite, that is below `1.8e19`.
pub(crate) fn magnitude(re: &[f32], im: &[f32], output: &mut [f32]) -> usize {
    dispatch!(Level::new(), simd => magnitude_kernel(simd, re, im, output))
}

#[inline(always)]
pub(super) fn magnitude_kernel<S: Simd>(
    simd: S,
    re: &[f32],
    im: &[f32],
    output: &mut [f32],
) -> usize {
    zip_map(simd, [re, im], output, |x, y| x.mul_add(x, y.mul(y)).sqrt())
}
