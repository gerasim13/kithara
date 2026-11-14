use fearless_simd::Simd;

// The token supplies the selected target features to LLVM's element-wise loop.
#[inline(always)]
pub(crate) fn diffuse_kernel<S: Simd>(
    _simd: S,
    samples: &mut [f32],
    history: &[f32],
    gain: f32,
    writes: &mut [f32],
) -> usize {
    let len = samples.len().min(history.len()).min(writes.len());
    for ((sample, delayed), write) in samples.iter_mut().zip(history).zip(writes) {
        *write = delayed.mul_add(gain, *sample);
        *sample = *delayed - *sample;
    }
    len
}

#[inline(always)]
pub(crate) fn blend_kernel<S: Simd>(
    _simd: S,
    newer: &[f32],
    older: &[f32],
    fraction: f32,
    output: &mut [f32],
) -> usize {
    let len = output.len().min(newer.len()).min(older.len());
    for ((output, newer), older) in output.iter_mut().zip(newer).zip(older) {
        *output = fraction.mul_add(*older - *newer, *newer);
    }
    len
}
