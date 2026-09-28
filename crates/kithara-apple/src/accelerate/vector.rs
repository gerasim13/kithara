use std::ptr;

use super::ffi::{DspSplitComplex, vDSP_maxmgv, vDSP_vmul, vDSP_zvabs};

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

/// `output[i] = a[i] · b[i]` over the common prefix of the three slices;
/// returns its length.
#[must_use]
pub fn multiply_f32(a: &[f32], b: &[f32], output: &mut [f32]) -> usize {
    let len = a.len().min(b.len()).min(output.len());
    if len == 0 {
        return 0;
    }
    // SAFETY: a, b and output each hold at least len floats at stride one.
    // SAFETY: output is an exclusive borrow, so it overlaps neither input.
    unsafe { vDSP_vmul(a.as_ptr(), 1, b.as_ptr(), 1, output.as_mut_ptr(), 1, len) };
    len
}

/// `output[i] = |re[i] + i·im[i]|` over the common prefix of the three
/// slices; returns its length.
#[must_use]
pub fn magnitude_f32(re: &[f32], im: &[f32], output: &mut [f32]) -> usize {
    let len = re.len().min(im.len()).min(output.len());
    if len == 0 {
        return 0;
    }
    let split = DspSplitComplex {
        realp: re.as_ptr().cast_mut(),
        imagp: im.as_ptr().cast_mut(),
    };
    // SAFETY: split points at len readable floats per plane and vDSP_zvabs only reads it.
    // SAFETY: output holds len writable floats and, as an exclusive borrow, overlaps neither plane.
    unsafe { vDSP_zvabs(ptr::from_ref(&split), 1, output.as_mut_ptr(), 1, len) };
    len
}
