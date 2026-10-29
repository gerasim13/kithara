use std::{ffi::c_int, ptr};

use super::ffi::{
    vDSP_conv, vDSP_maxmgv, vDSP_svesq, vDSP_vadd, vDSP_vmul, vDSP_vsma, vDSP_vsmsa, vDSP_vsub,
    vvatan2f,
};

/// `output[i] = atan2(im[i], re[i])` in radians over the common prefix;
/// returns its length and leaves the remaining output untouched.
///
/// # Panics
///
/// Only if an internal length invariant fails: Apple's `usize` holds
/// `c_int::MAX`, and each chunk is bounded by that positive maximum.
#[must_use]
pub fn phase_f32(re: &[f32], im: &[f32], output: &mut [f32]) -> usize {
    let len = re.len().min(im.len()).min(output.len());
    if len == 0 {
        return 0;
    }
    let chunk_size = usize::try_from(c_int::MAX).expect("invariant: Apple usize holds c_int::MAX");
    let (mut re, mut im, mut output) = (&re[..len], &im[..len], &mut output[..len]);
    while !output.is_empty() {
        let count = output.len().min(chunk_size);
        let (re_chunk, re_tail) = re.split_at(count);
        let (im_chunk, im_tail) = im.split_at(count);
        let (output_chunk, output_tail) = output.split_at_mut(count);
        let n = c_int::try_from(count).expect("invariant: each chunk is bounded by c_int::MAX");
        // SAFETY: all three nonempty chunks hold n contiguous f32 values.
        // SAFETY: output is exclusive and overlaps neither input; inputs may share storage.
        // SAFETY: n is a valid positive C int for the synchronous call; atan2 takes im before re.
        unsafe {
            vvatan2f(
                output_chunk.as_mut_ptr(),
                im_chunk.as_ptr(),
                re_chunk.as_ptr(),
                ptr::from_ref(&n),
            );
        }
        (re, im, output) = (re_tail, im_tail, output_tail);
    }
    len
}

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

/// `output[k] = Σ_j signal[k + j]·kernel[j]` for every lag `k` at which the
/// kernel fits inside `signal`, as many as `output` holds; returns the lag
/// count.
#[must_use]
pub fn correlate_f32(signal: &[f32], kernel: &[f32], output: &mut [f32]) -> usize {
    let taps = kernel.len();
    let lags = signal
        .len()
        .saturating_add(1)
        .saturating_sub(taps)
        .min(output.len());
    if taps == 0 || lags == 0 {
        return 0;
    }
    // SAFETY: signal holds lags + taps − 1 readable floats, kernel taps, output lags writable ones.
    // SAFETY: a positive kernel stride makes vDSP_conv correlate rather than convolve.
    // SAFETY: output is an exclusive borrow, so it overlaps neither input.
    unsafe {
        vDSP_conv(
            signal.as_ptr(),
            1,
            kernel.as_ptr(),
            1,
            output.as_mut_ptr(),
            1,
            lags,
            taps,
        );
    }
    lags
}

/// `Σx²` over `samples`; `0.0` for an empty slice.
#[must_use]
pub fn sum_squares_f32(samples: &[f32]) -> f32 {
    let mut sum = 0.0;
    if samples.is_empty() {
        return sum;
    }
    // SAFETY: samples points at samples.len() contiguous f32 values with stride one.
    // SAFETY: sum is a valid f32 destination for the synchronous call.
    unsafe { vDSP_svesq(samples.as_ptr(), 1, ptr::from_mut(&mut sum), samples.len()) };
    sum
}

/// `x[i] = x[i]*scale + offset` in place; returns the length of `x`.
#[must_use]
pub fn affine_f32(x: &mut [f32], scale: f32, offset: f32) -> usize {
    if x.is_empty() {
        return 0;
    }
    let samples = x.as_mut_ptr();
    // SAFETY: samples holds x.len() floats at stride one; vDSP reads and
    // SAFETY: writes the same element in place.
    // SAFETY: scale and offset are valid f32 sources for the synchronous call.
    unsafe {
        vDSP_vsmsa(
            samples.cast_const(),
            1,
            ptr::from_ref(&scale),
            ptr::from_ref(&offset),
            samples,
            1,
            x.len(),
        );
    }
    x.len()
}

/// `x[i] -= y[i]` over the common prefix of the two slices; returns its
/// length.
#[must_use]
pub fn sub_f32(x: &mut [f32], y: &[f32]) -> usize {
    let len = x.len().min(y.len());
    if len == 0 {
        return 0;
    }
    let samples = x.as_mut_ptr();
    // SAFETY: samples and y each hold at least len floats at stride one.
    // SAFETY: x is an exclusive borrow, so y never overlaps it; vDSP reads and
    // SAFETY: writes each element of x in place.
    unsafe { vDSP_vsub(y.as_ptr(), 1, samples.cast_const(), 1, samples, 1, len) };
    len
}

/// `acc[i] += x[i]*gain` over the common prefix of the two slices; returns
/// its length.
#[must_use]
pub fn scale_add_f32(x: &[f32], gain: f32, acc: &mut [f32]) -> usize {
    let len = x.len().min(acc.len());
    if len == 0 {
        return 0;
    }
    let sums = acc.as_mut_ptr();
    // SAFETY: x and sums each hold at least len floats at stride one.
    // SAFETY: acc is an exclusive borrow, so x never overlaps it; vDSP reads and
    // SAFETY: writes each element of acc in place.
    // SAFETY: gain is a valid f32 source for the synchronous call.
    unsafe {
        vDSP_vsma(
            x.as_ptr(),
            1,
            ptr::from_ref(&gain),
            sums.cast_const(),
            1,
            sums,
            1,
            len,
        );
    }
    len
}

/// `acc[i] += x[i]` over the common prefix of the two slices; returns its
/// length.
#[must_use]
pub fn add_f32(x: &[f32], acc: &mut [f32]) -> usize {
    let len = x.len().min(acc.len());
    if len == 0 {
        return 0;
    }
    let sums = acc.as_mut_ptr();
    // SAFETY: x and sums each hold at least len floats at stride one.
    // SAFETY: acc is an exclusive borrow, so x never overlaps it; vDSP reads and
    // SAFETY: writes each element of acc in place.
    unsafe { vDSP_vadd(x.as_ptr(), 1, sums.cast_const(), 1, sums, 1, len) };
    len
}
