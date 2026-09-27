use std::num::NonZeroUsize;

/// Writes `plane` into every `stride`-th slot of `output` from slot 0, bit for
/// bit; the last frame may be partial (`output.len().div_ceil(stride)` frames
/// fit). Returns the frames written.
pub(crate) fn scatter(plane: &[f32], output: &mut [f32], stride: NonZeroUsize) -> usize {
    let frames = plane.len().min(output.len().div_ceil(stride.get()));
    for (slot, sample) in output.iter_mut().step_by(stride.get()).zip(plane) {
        *slot = *sample;
    }
    frames
}

/// Reads every `stride`-th slot of `input` from slot 0 into `plane`, bit for
/// bit; the last frame may be partial, as in [`scatter`]. Returns the frames
/// read.
pub(crate) fn gather(input: &[f32], stride: NonZeroUsize, plane: &mut [f32]) -> usize {
    let frames = plane.len().min(input.len().div_ceil(stride.get()));
    for (slot, sample) in plane.iter_mut().zip(input.iter().step_by(stride.get())) {
        *slot = *sample;
    }
    frames
}
