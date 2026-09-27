use fearless_simd::{Level, dispatch, prelude::*};

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub(crate) use super::cascade::Cascade;
use super::simd::padded;

pub(crate) fn deinterleave_pair(input: &[f32], left: &mut [f32], right: &mut [f32]) -> usize {
    dispatch!(Level::new(), simd => deinterleave_pair_kernel(simd, input, left, right))
}

pub(crate) fn interleave_pair(left: &[f32], right: &[f32], output: &mut [f32]) -> usize {
    dispatch!(Level::new(), simd => interleave_pair_kernel(simd, left, right, output))
}

#[inline(always)]
pub(super) fn interleave_pair_kernel<S: Simd>(
    simd: S,
    left: &[f32],
    right: &[f32],
    output: &mut [f32],
) -> usize {
    let frames = left.len().min(right.len()).min(output.len() / 2);
    let (Some(left), Some(right), Some(pairs)) = (
        left.get(..frames),
        right.get(..frames),
        output.as_chunks_mut::<2>().0.get_mut(..frames),
    ) else {
        return 0;
    };
    let lanes = S::f32s::LEN;
    let mut lefts = left.chunks_exact(lanes);
    let mut rights = right.chunks_exact(lanes);
    let mut outputs = pairs.chunks_exact_mut(lanes);
    for ((l, r), out) in (&mut lefts).zip(&mut rights).zip(&mut outputs) {
        let (lo, hi) = S::f32s::from_slice(simd, l).interleave(S::f32s::from_slice(simd, r));
        let (out_lo, out_hi) = out.as_flattened_mut().split_at_mut(lanes);
        lo.store_slice(out_lo);
        hi.store_slice(out_hi);
    }
    let (lo, hi) = padded(simd, lefts.remainder()).interleave(padded(simd, rights.remainder()));
    for (slot, sample) in outputs
        .into_remainder()
        .as_flattened_mut()
        .iter_mut()
        .zip(lo.as_slice().iter().chain(hi.as_slice()))
    {
        *slot = *sample;
    }
    frames
}

#[inline(always)]
pub(super) fn deinterleave_pair_kernel<S: Simd>(
    simd: S,
    input: &[f32],
    left: &mut [f32],
    right: &mut [f32],
) -> usize {
    let frames = (input.len() / 2).min(left.len()).min(right.len());
    let (Some(pairs), Some(left), Some(right)) = (
        input.as_chunks::<2>().0.get(..frames),
        left.get_mut(..frames),
        right.get_mut(..frames),
    ) else {
        return 0;
    };
    let lanes = S::f32s::LEN;
    let mut inputs = pairs.chunks_exact(lanes);
    let mut lefts = left.chunks_exact_mut(lanes);
    let mut rights = right.chunks_exact_mut(lanes);
    for ((block, l), r) in (&mut inputs).zip(&mut lefts).zip(&mut rights) {
        let (lo, hi) = block.as_flattened().split_at(lanes);
        let (even, odd) = S::f32s::from_slice(simd, lo).deinterleave(S::f32s::from_slice(simd, hi));
        even.store_slice(l);
        odd.store_slice(r);
    }
    let tail = inputs.remainder().as_flattened();
    let (lo, hi) = tail.split_at(tail.len().min(lanes));
    let (even, odd) = padded(simd, lo).deinterleave(padded(simd, hi));
    for (slot, sample) in lefts.into_remainder().iter_mut().zip(even.as_slice()) {
        *slot = *sample;
    }
    for (slot, sample) in rights.into_remainder().iter_mut().zip(odd.as_slice()) {
        *slot = *sample;
    }
    frames
}

pub(crate) fn peak(samples: &[f32]) -> f32 {
    dispatch!(Level::new(), simd => peak_kernel(simd, samples))
}

#[inline(always)]
pub(super) fn peak_kernel<S: Simd>(simd: S, samples: &[f32]) -> f32 {
    let mut blocks = samples.chunks_exact(S::f32s::LEN);
    let mut peak = S::f32s::splat(simd, 0.0);
    for block in &mut blocks {
        peak = peak.max(S::f32s::from_slice(simd, block).abs());
    }
    peak.max(padded(simd, blocks.remainder()).abs())
        .reduce_max()
}
