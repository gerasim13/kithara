use std::ptr;

use super::ffi::{DspComplex, DspSplitComplex, VdspStride, vDSP_ctoz, vDSP_vasm, vDSP_ztoc};

mod consts {
    use super::VdspStride;

    /// Float stride between consecutive `DspComplex` pairs in an interleaved buffer.
    pub(super) const PAIR_STRIDE: VdspStride = 2;

    /// Scale that turns the sum of a pair into its mean.
    pub(super) const HALF: f32 = 0.5;
}

/// Interleaves `left` and `right` into `output` as `[l0, r0, l1, r1, …]`.
///
/// Writes the common prefix of the three slices and returns its length in frames.
#[must_use]
pub fn interleave_pair_f32(left: &[f32], right: &[f32], output: &mut [f32]) -> usize {
    let frames = left.len().min(right.len()).min(output.len() / 2);
    if frames == 0 {
        return 0;
    }
    let split = DspSplitComplex {
        realp: left.as_ptr().cast_mut(),
        imagp: right.as_ptr().cast_mut(),
    };
    // SAFETY: `split` points at `frames` readable floats per plane and vDSP_ztoc only reads it.
    // SAFETY: `output` holds `2 * frames` floats written as `DspComplex` pairs at float stride 2.
    // SAFETY: `DspComplex` is two `f32` fields, so it needs only `f32` alignment.
    unsafe {
        vDSP_ztoc(
            &split,
            1,
            output.as_mut_ptr().cast::<DspComplex>(),
            consts::PAIR_STRIDE,
            frames,
        );
    }
    frames
}

/// Splits `[l0, r0, l1, r1, …]` from `input` into `left` and `right`.
///
/// Reads whole pairs of the common prefix and returns their count.
#[must_use]
pub fn deinterleave_pair_f32(input: &[f32], left: &mut [f32], right: &mut [f32]) -> usize {
    let frames = (input.len() / 2).min(left.len()).min(right.len());
    if frames == 0 {
        return 0;
    }
    let split = DspSplitComplex {
        realp: left.as_mut_ptr(),
        imagp: right.as_mut_ptr(),
    };
    // SAFETY: `input` holds `2 * frames` floats read as `DspComplex` pairs at float stride 2.
    // SAFETY: `split` points at `frames` writable floats per plane.
    // SAFETY: the planes and `input` are distinct borrows, so they never overlap.
    unsafe {
        vDSP_ctoz(
            input.as_ptr().cast::<DspComplex>(),
            consts::PAIR_STRIDE,
            &split,
            1,
            frames,
        );
    }
    frames
}

/// Averages each `[l, r]` pair of `input` into `mono` as `(l + r) · 0.5`.
///
/// Reads whole pairs of the common prefix and returns their count.
#[must_use]
pub fn downmix_pair_f32(input: &[f32], mono: &mut [f32]) -> usize {
    let frames = (input.len() / 2).min(mono.len());
    if frames == 0 {
        return 0;
    }
    let half = consts::HALF;
    // SAFETY: `input` holds `2 * frames` floats: lefts at even offsets, rights at odd ones, both read at float stride 2.
    // SAFETY: `half` is one readable `f32`, and `mono` holds `frames` writable floats that no input borrow overlaps.
    unsafe {
        vDSP_vasm(
            input.as_ptr(),
            consts::PAIR_STRIDE,
            input.as_ptr().wrapping_add(1),
            consts::PAIR_STRIDE,
            ptr::from_ref(&half),
            mono.as_mut_ptr(),
            1,
            frames,
        );
    }
    frames
}
