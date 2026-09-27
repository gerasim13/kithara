use std::num::NonZeroUsize;

/// Writes `plane` into every `stride`-th slot of `output` from slot 0, bit for
/// bit; the last frame may be partial (`output.len().div_ceil(stride)` frames
/// fit). Returns the frames written.
pub(crate) fn scatter(plane: &[f32], output: &mut [f32], stride: NonZeroUsize) -> usize {
    let (whole, frames) = (output.len() / stride, output.len().div_ceil(stride.get()));
    for (frame, sample) in output.chunks_exact_mut(stride.get()).zip(plane) {
        if let Some(slot) = frame.first_mut() {
            *slot = *sample;
        }
    }
    let partial = output.chunks_exact_mut(stride.get()).into_remainder();
    if let (Some(slot), Some(sample)) = (partial.first_mut(), plane.get(whole)) {
        *slot = *sample;
    }
    plane.len().min(frames)
}

/// Reads every `stride`-th slot of `input` from slot 0 into `plane`, bit for
/// bit; the last frame may be partial, as in [`scatter`]. Returns the frames
/// read.
pub(crate) fn gather(input: &[f32], stride: NonZeroUsize, plane: &mut [f32]) -> usize {
    let frames = input.chunks_exact(stride.get());
    let partial = frames.remainder().first();
    for (slot, frame) in plane.iter_mut().zip(frames) {
        if let Some(sample) = frame.first() {
            *slot = *sample;
        }
    }
    if let (Some(slot), Some(sample)) = (plane.get_mut(input.len() / stride), partial) {
        *slot = *sample;
    }
    plane.len().min(input.len().div_ceil(stride.get()))
}
