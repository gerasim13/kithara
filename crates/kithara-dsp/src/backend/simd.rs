use fearless_simd::{Level, dispatch, prelude::*};

/// Replaces `NaN`, ±infinity, subnormals and −0.0 with `+0.0` in place; every
/// other value keeps its bits.
pub fn sanitize(samples: &mut [f32]) {
    dispatch!(Level::new(), simd => sanitize_kernel(simd, samples));
}

#[inline(always)]
pub(super) fn sanitize_kernel<S: Simd>(simd: S, samples: &mut [f32]) {
    let smallest = S::f32s::splat(simd, f32::MIN_POSITIVE);
    let largest = S::f32s::splat(simd, f32::MAX);
    let zero = S::f32s::splat(simd, 0.0);
    let mut blocks = samples.chunks_exact_mut(S::f32s::LEN);
    for block in &mut blocks {
        sanitized::<S>(S::f32s::from_slice(simd, block), smallest, largest, zero)
            .store_slice(block);
    }
    let tail = blocks.into_remainder();
    let cleaned = sanitized::<S>(padded(simd, tail), smallest, largest, zero);
    for (slot, sample) in tail.iter_mut().zip(cleaned.as_slice()) {
        *slot = *sample;
    }
}

/// `x` where `MIN_POSITIVE <= |x| <= MAX`, `+0.0` elsewhere; `NaN` fails both tests.
#[inline(always)]
fn sanitized<S: Simd>(x: S::f32s, smallest: S::f32s, largest: S::f32s, zero: S::f32s) -> S::f32s {
    let magnitude = x.abs();
    let normal = magnitude.simd_ge(smallest).select(x, zero);
    magnitude.simd_le(largest).select(normal, zero)
}

/// A vector holding `tail` in its first lanes and `+0.0` in the rest.
#[inline(always)]
pub(super) fn padded<S: Simd>(simd: S, tail: &[f32]) -> S::f32s {
    let mut vector = S::f32s::splat(simd, 0.0);
    for (slot, sample) in vector.as_mut_slice().iter_mut().zip(tail) {
        *slot = *sample;
    }
    vector
}
