use fearless_simd::prelude::*;

/// The vector holding `block`, zero-padded when it is shorter than the lanes.
#[inline(always)]
pub(crate) fn vector<S: Simd>(simd: S, block: &[f32]) -> S::f32s {
    if block.len() == S::f32s::LEN {
        S::f32s::from_slice(simd, block)
    } else {
        padded(simd, block)
    }
}

/// Writes the first `block.len()` lanes of `value` into `block`.
#[inline(always)]
pub(crate) fn store<S: Simd>(value: S::f32s, block: &mut [f32]) {
    if block.len() == S::f32s::LEN {
        value.store_slice(block);
    } else {
        for (slot, lane) in block.iter_mut().zip(value.as_slice()) {
            *slot = *lane;
        }
    }
}

/// A vector holding `tail` in its first lanes and `+0.0` in the rest.
#[inline(always)]
pub(crate) fn padded<S: Simd>(simd: S, tail: &[f32]) -> S::f32s {
    let mut vector = S::f32s::splat(simd, 0.0);
    for (slot, sample) in vector.as_mut_slice().iter_mut().zip(tail) {
        *slot = *sample;
    }
    vector
}
