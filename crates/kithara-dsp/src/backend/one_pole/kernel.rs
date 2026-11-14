use std::ops::Mul;

use fearless_simd::prelude::*;

use crate::backend::simd::{store, vector};

#[inline(always)]
pub(crate) fn low_pass_kernel<S: Simd, const CHANNELS: usize>(
    simd: S,
    coefficient: f32,
    state: &mut [f32; CHANNELS],
    planes: &mut [Option<&mut [f32]>; CHANNELS],
    frames: usize,
) -> usize {
    let feedback = S::f32s::splat(simd, coefficient);
    let feedforward = S::f32s::splat(simd, 1.0 - coefficient);
    let mut channels = planes
        .each_mut()
        .map(|plane| plane.as_deref_mut().unwrap_or_default().iter_mut());
    for (channels, memory) in channels
        .chunks_mut(S::f32s::LEN)
        .zip(state.chunks_mut(S::f32s::LEN))
    {
        let mut previous = vector(simd, memory);
        for _ in 0..frames {
            let mut samples: [Option<&mut f32>; CHANNELS] =
                std::array::from_fn(|lane| channels.get_mut(lane).and_then(Iterator::next));
            let input = S::f32s::from_fn(simd, |lane| {
                samples
                    .get(lane)
                    .and_then(Option::as_deref)
                    .copied()
                    .unwrap_or(0.0)
            });
            previous = previous.mul_add_precise(feedback, input.mul(feedforward));
            for (sample, output) in samples.iter_mut().zip(previous.as_slice()) {
                if let Some(sample) = sample {
                    **sample = *output;
                }
            }
        }
        store::<S>(previous, memory);
    }
    frames
}
